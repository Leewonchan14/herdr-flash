#!/usr/bin/env python3
"""End-to-end proof for herdr-flash in an isolated named Herdr session.

Drives a real Herdr client in a PTY: types a fixture into a pane, invokes the plugin popup with
prefix+shift+s, types a query, picks a hint, selects a word and yanks it. Asserts:

  * the popup rendered the captured pane text,
  * the yank reached the client as an OSC 52 sequence carrying the expected text,
  * the plugin's own state log recorded the copy outcome,
  * the popup closed and the session stayed alive.

Never touches the default session: it uses `--session herdr-flash-lab` only.
"""

import base64
import json
import os
import pty
import re
import select
import shutil
import signal
import subprocess
import sys
import time

SESSION = "herdr-flash-lab"

ANSI_RE = re.compile(
    r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)"   # OSC (incl. the OSC 52 payload)
    r"|\x1b\[[0-9;?]*[ -/]*[@-~]"              # CSI
    r"|\x1b[()][A-Z0-9]"                       # charset selection
)
STATE_GLOB_DIR = os.path.expanduser("~/.local/state/herdr/plugins")
EXPECTED = "example"
FIXTURE = "alpha beta gamma\nURL https://example.com/path/to/file\nEND\n"


class Lab:
    def __init__(self):
        self.master, slave = pty.openpty()
        import fcntl
        import struct
        import termios

        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        env = dict(os.environ)
        env["TERM"] = "xterm-256color"
        env.pop("HERDR_SOCKET_PATH", None)
        self.proc = subprocess.Popen(
            ["herdr", "--session", SESSION],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=env,
            close_fds=True,
        )
        os.close(slave)
        self.output = bytearray()

    def pump(self, seconds):
        deadline = time.time() + seconds
        while time.time() < deadline:
            ready, _, _ = select.select([self.master], [], [], 0.1)
            if not ready:
                continue
            try:
                chunk = os.read(self.master, 65536)
            except OSError:
                break
            if not chunk:
                break
            self.output.extend(chunk)

    def plain(self):
        """Client stream with escape sequences removed: the rendered screen text."""
        return ANSI_RE.sub(" ", self.output.decode("utf-8", "replace"))

    def wait_for_text(self, token, timeout=8.0):
        """Pump the client stream until the rendered screen contains `token`."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            if token in self.plain():
                return True
            self.pump(0.25)
        return False

    def send(self, keys, settle=0.6):
        os.write(self.master, keys.encode())
        self.pump(settle)

    def cli(self, *args):
        env = dict(os.environ)
        env["HERDR_SESSION"] = SESSION
        return subprocess.run(
            ["herdr", "--session", SESSION, *args],
            capture_output=True,
            text=True,
            env=env,
            timeout=30,
        )

    def close(self):
        try:
            self.cli("server", "stop")
        except Exception:
            pass
        self.proc.send_signal(signal.SIGTERM)
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        try:
            os.close(self.master)
        except OSError:
            pass


def state_log_text():
    for entry in os.listdir(STATE_GLOB_DIR):
        if "herdr-flash" in entry:
            path = os.path.join(STATE_GLOB_DIR, entry, "herdr-flash.log")
            if os.path.exists(path):
                return open(path).read()
    return ""


def main():
    if shutil.which("herdr") is None:
        print("herdr not on PATH", file=sys.stderr)
        return 2

    log_before = state_log_text()
    saved_clipboard = subprocess.run(["pbpaste"], capture_output=True, text=True).stdout
    lab = Lab()
    failures = []
    try:
        # Wait for the session server to answer.
        for _ in range(60):
            probe = lab.cli("pane", "list")
            if probe.returncode == 0 and "pane_id" in probe.stdout:
                break
            time.sleep(0.5)
        else:
            failures.append("lab session never became reachable")
            raise SystemExit(report(failures, lab))

        plugins = lab.cli("plugin", "list").stdout
        if "Leewonchan14.herdr-flash" not in plugins:
            failures.append("herdr-flash is not visible in the lab session")

        lab.pump(1.0)
        # Fixture text into the focused pane's shell.
        lab.send("printf '%s' '" + FIXTURE.replace("\n", "\\n") + "'\r", settle=2.0)
        listing = lab.cli("pane", "list").stdout
        pane_ids = re.findall(r'"pane_id":\s*"([^"]+)"', listing)
        visible = lab.cli("pane", "read", pane_ids[0], "--source", "visible").stdout if pane_ids else ""
        if EXPECTED not in visible:
            failures.append(f"fixture text did not reach the pane viewport: {visible[:200]!r}")

        # prefix (ctrl+a) + s -> flash popup.
        lab.send("\x01S", settle=1.0)
        if not lab.wait_for_text("herdr-flash"):
            failures.append("the flash popup never opened")
        if not lab.wait_for_text(EXPECTED):
            failures.append("popup did not render the captured pane text")

        # Query "ex". The 'a' that follows a match stays typeable, so the labels in play are 's'
        # (the output hit nearest the prompt) and 'd' (the same text in the echoed command).
        lab.send("ex", settle=0.5)
        if not lab.wait_for_text("hints s d"):
            failures.append("status line did not render the hint keys")
        lab.send("s", settle=0.5)
        lab.send("ve", settle=0.5)
        lab.send("y", settle=1.5)

        text = lab.output.decode("utf-8", "replace")
        encoded = base64.b64encode(EXPECTED.encode()).decode()
        forwarded = f"\x1b]52;c;{encoded}\x07" in text
        # Local sessions prefer the native clipboard; SSH sessions get the forwarded sequence.
        clipboard = subprocess.run(["pbpaste"], capture_output=True, text=True).stdout
        if not forwarded and clipboard != EXPECTED:
            failures.append(
                f"yank did not land: osc52={forwarded} clipboard={clipboard!r}"
            )

        log_after = state_log_text()
        new_lines = log_after[len(log_before):] if log_after.startswith(log_before) else log_after
        if "outcome=copy" not in new_lines:
            failures.append(f"plugin state log has no copy outcome: {new_lines!r}")
        if not re.search(r"outcome=copy chars=7", new_lines):
            failures.append(f"unexpected copy length: {new_lines!r}")

        alive = lab.cli("pane", "list")
        if alive.returncode != 0:
            failures.append("lab session died during the run")
    finally:
        code = report(failures, lab)
        lab.close()
        if saved_clipboard:
            subprocess.run(["pbcopy"], input=saved_clipboard, text=True)
    return code


def report(failures, lab):
    log = state_log_text()
    print("=== plugin state log (tail) ===")
    print("\n".join(log.splitlines()[-6:]))
    if failures:
        print("=== FAILURES ===")
        for failure in failures:
            print("-", failure)
        tail = bytes(lab.output[-4000:]).decode("utf-8", "replace")
        print("=== client stream tail ===")
        print(repr(tail[-2000:]))
        return 1
    print("=== E2E OK: popup capture -> hint jump -> selection -> OSC 52 yank ===")
    return 0


if __name__ == "__main__":
    sys.exit(main())
