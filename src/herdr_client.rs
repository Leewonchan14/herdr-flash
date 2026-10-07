//! Herdr Unix-socket JSON-RPC client.
//!
//! Only read paths, the copy toast and the copy-mode handoff are used: the picker never moves the
//! source pane, never writes to its PTY, and never changes its scroll position. `pane.read` is the
//! capture, `pane.layout` supplies the wrap width for the captured text, and — on a Herdr that
//! implements it — `pane.copy_mode_jump` hands the picked cell to Herdr's own copy mode.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

const RPC_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisiblePane {
    pub text: String,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneScroll {
    pub offset_from_bottom: u64,
}

/// A request to place Herdr's copy-mode cursor on a viewport cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyModeJump {
    pub pane_id: String,
    /// Row within the pane's visible viewport, which is the picker's own row index.
    pub viewport_row: u32,
    /// Display cell within that row, which is the picker's own column.
    pub viewport_col: u16,
    pub content_revision: Option<u64>,
    pub offset_from_bottom: Option<u64>,
    /// Visible text of the target row as captured, so Herdr accepts the jump while unrelated
    /// output keeps churning.
    pub expected_row: Option<String>,
}

#[derive(Debug)]
pub struct SocketClient {
    socket_path: PathBuf,
    next_id: u64,
}

impl SocketClient {
    pub fn connect(socket_path: &Path) -> Result<Self> {
        // Fail fast when the socket is unusable, but keep the path for per-call connections.
        UnixStream::connect(socket_path).with_context(|| {
            format!(
                "cannot connect to the Herdr API socket at {}",
                socket_path.display()
            )
        })?;
        Ok(Self {
            socket_path: socket_path.to_path_buf(),
            next_id: 1,
        })
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = format!("herdr-flash:{}", self.next_id);
        self.next_id += 1;
        let request = json!({ "id": id, "method": method, "params": params });
        let mut stream = UnixStream::connect(&self.socket_path).with_context(|| {
            format!(
                "cannot connect to the Herdr API socket at {}",
                self.socket_path.display()
            )
        })?;
        stream
            .set_read_timeout(Some(RPC_TIMEOUT))
            .context("failed to set socket read timeout")?;
        stream
            .set_write_timeout(Some(RPC_TIMEOUT))
            .context("failed to set socket write timeout")?;
        let mut payload = serde_json::to_string(&request)?;
        payload.push('\n');
        stream
            .write_all(payload.as_bytes())
            .context("failed to write the Herdr request")?;
        stream
            .flush()
            .context("failed to flush the Herdr request")?;

        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .with_context(|| format!("failed to read the {method} response"))?;
        if line.trim().is_empty() {
            bail!("{method} returned no response");
        }
        let response: Value = serde_json::from_str(&line)
            .with_context(|| format!("{method} returned invalid JSON"))?;
        if let Some(error) = response.get("error") {
            let code = error["code"].as_str().unwrap_or("unknown");
            let message = error["message"].as_str().unwrap_or("no message");
            bail!("Herdr API error {code}: {message}");
        }
        response
            .get("result")
            .cloned()
            .with_context(|| format!("{method} returned no result"))
    }

    /// Focused pane from `pane.current`, used when the plugin context has no pane.
    pub fn focused_pane_id(&mut self) -> Result<String> {
        let result = self.call("pane.current", json!({}))?;
        result["pane"]["pane_id"]
            .as_str()
            .map(str::to_string)
            .context("pane.current did not include a pane id")
    }

    pub fn read_visible_pane(&mut self, pane_id: &str) -> Result<VisiblePane> {
        self.read_pane(pane_id, "visible")
    }

    /// The pane's recent output as logical lines.
    ///
    /// `visible` returns wrapped *screen* rows, which cannot express where the terminal wrapped;
    /// this read is the reference `Buffer::merge_soft_wraps` aligns against to recover soft wraps.
    pub fn read_unwrapped_pane(&mut self, pane_id: &str) -> Result<VisiblePane> {
        self.read_pane(pane_id, "recent_unwrapped")
    }

    fn read_pane(&mut self, pane_id: &str, source: &str) -> Result<VisiblePane> {
        let result = self.call(
            "pane.read",
            json!({
                "pane_id": pane_id,
                "source": source,
                "format": "text",
                "strip_ansi": true
            }),
        )?;
        let actual = result["type"].as_str().unwrap_or("<missing>");
        if actual != "pane_read" {
            bail!("expected a pane_read result, got {actual}");
        }
        let read = &result["read"];
        Ok(VisiblePane {
            text: read["text"].as_str().unwrap_or_default().to_string(),
            revision: read["revision"]
                .as_u64()
                .context("pane_read result did not include a revision")?,
        })
    }

    /// Width of the pane rectangle; the capture's wrap width is one cell narrower.
    pub fn visible_pane_width(&mut self, pane_id: &str) -> Result<usize> {
        let result = self.call("pane.layout", json!({ "pane_id": pane_id }))?;
        let actual = result["type"].as_str().unwrap_or("<missing>");
        if actual != "pane_layout" {
            bail!("expected a pane_layout result, got {actual}");
        }
        let panes = result["layout"]["panes"]
            .as_array()
            .context("pane_layout result did not include panes")?;
        let pane = panes
            .iter()
            .find(|pane| pane["pane_id"].as_str() == Some(pane_id))
            .context("pane_layout result did not include the requested pane")?;
        let width = pane["rect"]["width"]
            .as_u64()
            .context("pane_layout result did not include the pane width")?;
        usize::try_from(width).context("pane width did not fit in usize")
    }

    /// Current scroll offset, part of the viewport identity sent with a jump.
    pub fn pane_scroll(&mut self, pane_id: &str) -> Result<PaneScroll> {
        let result = self.call("pane.get", json!({ "pane_id": pane_id }))?;
        let actual = result["type"].as_str().unwrap_or("<missing>");
        if actual != "pane_info" {
            bail!("expected a pane_info result, got {actual}");
        }
        let offset = result["pane"]["scroll"]["offset_from_bottom"]
            .as_u64()
            .context("pane_info result did not include the scroll offset")?;
        Ok(PaneScroll {
            offset_from_bottom: offset,
        })
    }

    /// Ask Herdr to enter its copy mode with the cursor on a viewport cell of `pane_id`.
    ///
    /// The jump is sent while the popup is still open: Herdr defers it until the popup closes, so
    /// a rejection still reaches the picker instead of failing after the popup is gone.
    pub fn copy_mode_jump(&mut self, request: &CopyModeJump) -> Result<()> {
        let mut params = json!({
            "pane_id": request.pane_id,
            "viewport_row": request.viewport_row,
            "viewport_col": request.viewport_col,
        });
        if let Some(revision) = request.content_revision {
            params["content_revision"] = json!(revision);
        }
        if let Some(offset) = request.offset_from_bottom {
            params["offset_from_bottom"] = json!(offset);
        }
        if let Some(expected_row) = request.expected_row.as_deref() {
            params["expected_row"] = json!(expected_row);
        }
        self.call("pane.copy_mode_jump", params)?;
        Ok(())
    }

    /// True when the running server exposes `pane.copy_mode_jump`.
    ///
    /// Probes with an empty pane id: a server without the method answers `unknown variant`, while
    /// a server with it answers `pane_not_found`.
    pub fn supports_copy_mode_jump(&mut self) -> bool {
        match self.call(
            "pane.copy_mode_jump",
            json!({ "pane_id": "", "viewport_row": 0, "viewport_col": 0 }),
        ) {
            Ok(_) => true,
            Err(error) => !error.to_string().contains("unknown variant"),
        }
    }

    /// Show a Herdr toast. Failures are non-fatal for the caller.
    pub fn show_notification(&mut self, title: &str) -> Result<()> {
        self.call("notification.show", json!({ "title": title }))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufWriter;
    use std::os::unix::net::UnixListener;
    use std::thread;

    /// Serve one request and reply with `body`, returning the received request line.
    fn one_shot_server(body: &str) -> (PathBuf, thread::JoinHandle<String>) {
        let path = std::env::temp_dir().join(format!(
            "herdr-flash-test-{}-{}.sock",
            std::process::id(),
            rand_suffix()
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let body = body.to_string();
        let handle = thread::spawn(move || {
            // `SocketClient::connect` probes the socket, so skip connections without a request.
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                if reader.read_line(&mut request).unwrap_or(0) == 0 {
                    continue;
                }
                let mut writer = BufWriter::new(stream);
                writer.write_all(body.as_bytes()).unwrap();
                writer.write_all(b"\n").unwrap();
                writer.flush().unwrap();
                return request;
            }
            panic!("no request reached the fixture server");
        });
        (path, handle)
    }

    fn rand_suffix() -> u64 {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    #[test]
    fn read_visible_pane_shapes_the_request_and_parses_text() {
        let body =
            r#"{"id":"x","result":{"type":"pane_read","read":{"text":"hello","revision":7}}}"#;
        let (path, server) = one_shot_server(body);
        let mut client = SocketClient::connect(&path).unwrap();
        let pane = client.read_visible_pane("w1:p1").unwrap();
        assert_eq!(pane.text, "hello");
        assert_eq!(pane.revision, 7);
        let request: Value = serde_json::from_str(&server.join().unwrap()).unwrap();
        assert_eq!(request["method"], "pane.read");
        assert_eq!(request["params"]["pane_id"], "w1:p1");
        assert_eq!(request["params"]["source"], "visible");
        assert_eq!(request["params"]["strip_ansi"], true);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn read_unwrapped_pane_shapes_the_request() {
        let body =
            r#"{"id":"x","result":{"type":"pane_read","read":{"text":"one\ntwo","revision":8}}}"#;
        let (path, server) = one_shot_server(body);
        let mut client = SocketClient::connect(&path).unwrap();
        let pane = client.read_unwrapped_pane("w1:p1").unwrap();
        assert_eq!(pane.text, "one\ntwo");
        assert_eq!(pane.revision, 8);
        let request: Value = serde_json::from_str(&server.join().unwrap()).unwrap();
        assert_eq!(request["method"], "pane.read");
        assert_eq!(request["params"]["source"], "recent_unwrapped");
        assert_eq!(request["params"]["format"], "text");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn visible_pane_width_reads_the_layout_rect() {
        let body = r#"{"id":"x","result":{"type":"pane_layout","layout":{"panes":[{"pane_id":"w1:p2","rect":{"width":120}},{"pane_id":"w1:p1","rect":{"width":80}}]}}}"#;
        let (path, server) = one_shot_server(body);
        let mut client = SocketClient::connect(&path).unwrap();
        assert_eq!(client.visible_pane_width("w1:p1").unwrap(), 80);
        let request: Value = serde_json::from_str(&server.join().unwrap()).unwrap();
        assert_eq!(request["method"], "pane.layout");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn api_errors_surface_the_code_and_message() {
        let body =
            r#"{"id":"x","error":{"code":"pane_not_found","message":"pane w9:p9 not found"}}"#;
        let (path, server) = one_shot_server(body);
        let mut client = SocketClient::connect(&path).unwrap();
        let error = client.read_visible_pane("w9:p9").unwrap_err().to_string();
        assert!(error.contains("pane_not_found"), "{error}");
        assert!(error.contains("not found"), "{error}");
        let _ = server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn focused_pane_id_reads_pane_current() {
        let body = r#"{"id":"x","result":{"type":"pane_current","pane":{"pane_id":"w2:p3"}}}"#;
        let (path, server) = one_shot_server(body);
        let mut client = SocketClient::connect(&path).unwrap();
        assert_eq!(client.focused_pane_id().unwrap(), "w2:p3");
        let request: Value = serde_json::from_str(&server.join().unwrap()).unwrap();
        assert_eq!(request["method"], "pane.current");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn copy_mode_jump_sends_the_viewport_identity() {
        let body = r#"{"id":"x","result":{"type":"ok"}}"#;
        let (path, server) = one_shot_server(body);
        let mut client = SocketClient::connect(&path).unwrap();
        client
            .copy_mode_jump(&CopyModeJump {
                pane_id: "w1:p1".into(),
                viewport_row: 12,
                viewport_col: 5,
                content_revision: Some(7),
                offset_from_bottom: Some(3),
                expected_row: Some("hello world".into()),
            })
            .unwrap();
        let request: Value = serde_json::from_str(&server.join().unwrap()).unwrap();
        assert_eq!(request["method"], "pane.copy_mode_jump");
        assert_eq!(request["params"]["pane_id"], "w1:p1");
        assert_eq!(request["params"]["viewport_row"], 12);
        assert_eq!(request["params"]["viewport_col"], 5);
        assert_eq!(request["params"]["content_revision"], 7);
        assert_eq!(request["params"]["offset_from_bottom"], 3);
        assert_eq!(request["params"]["expected_row"], "hello world");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_server_without_the_method_is_not_supported() {
        let body = r#"{"id":"x","error":{"code":"invalid_request","message":"invalid request: unknown variant `pane.copy_mode_jump`, expected one of `ping`"}}"#;
        let (path, server) = one_shot_server(body);
        let mut client = SocketClient::connect(&path).unwrap();
        assert!(!client.supports_copy_mode_jump());
        let _ = server.join();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_server_with_the_method_is_supported_even_when_the_probe_pane_is_missing() {
        let body = r#"{"id":"x","error":{"code":"pane_not_found","message":"pane  not found"}}"#;
        let (path, server) = one_shot_server(body);
        let mut client = SocketClient::connect(&path).unwrap();
        assert!(client.supports_copy_mode_jump());
        let _ = server.join();
        let _ = std::fs::remove_file(path);
    }
}
