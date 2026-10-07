#!/usr/bin/env python3
"""End-to-end proof for the herdr-flash handoff to Herdr's own copy mode.

Drives a real Herdr client in a PTY against a Herdr that implements `pane.copy_mode_jump`
(see `docs/herdr-copy-mode-jump.patch`) and asserts the whole chain:

  A. `prefix+shift+s` -> flash popup over the captured pane -> query -> hint ->
     Herdr's own copy mode lands on that cell -> `v` `e` `y` yanks it through Herdr.
  B. Inside copy mode, the configured `copy_mode_key` (`shift+s`) opens the same picker.
  C. Against a Herdr without the API the picker keeps its own copy cursor instead of handing
     off, so the plugin still works on a stock build.

Only named lab sessions are used; the default session is never touched.

Environment:
  HERDR_BIN         Herdr binary that implements pane.copy_mode_jump
                    (default: the local patched build, else `herdr`)
  HERDR_STABLE_BIN  Herdr binary without the API, for case C (default: `herdr` on PATH)
"""

import fcntl
import os
import re
import pty
import select
import shutil
import signal
import struct
import subprocess
import sys
import termios
import time

LAB = "herdr-flash-lab"
LAB_STABLE = "herdr-flash-lab-stable"
EXPECTED = "example"
# A query whose match starts on the word start and whose trailing character ('.') is not a hint
# key, so the nearest match keeps the "a" label.
QUERY = "example"
FIXTURE = "alpha beta gamma\nURL https://example.com/path/to/file\nEND\n"
PATCHED = os.path.expanduser("~/Desktop/project/herdr/target/release/herdr")
STATE_DIR = os.path.expanduser("~/.local/state/herdr/plugins")
ANSI_RE = re.compile(
    r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)"  # OSC (incl. the OSC 52 payload)
    r"|\x1b\[[0-9;?]*[ -/]*[@-~]"  # CSI
    r"|\x1b[()][A-Z0-9]"  # charset selection
)


class Lab:
    def __init__(self, binary, session):
        self.binary = binary
        self.session = session
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        env = dict(os.environ)
        env["TERM"] = "xterm-256color"
        env.pop("HERDR_SOCKET_PATH", None)
        self.proc = subprocess.Popen(
            [binary, "--session", session],
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
        return ANSI_RE.sub(" ", self.output.decode("utf-8", "replace"))

    def wait_for_text(self, token, timeout=10.0):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if token in self.plain():
                return True
            self.pump(0.25)
        return False

    def wait_for_any(self, tokens, timeout=10.0):
        """True once any of `tokens` shows up: the status line is redrawn cell by cell, so a
        phrase can be split across the stream and only some of its fragments stay contiguous."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            plain = self.plain()
            if any(token in plain for token in tokens):
                return True
            self.pump(0.25)
        return False

    def send(self, keys, settle=0.6):
        os.write(self.master, keys.encode())
        self.pump(settle)

    def cli(self, *args):
        return subprocess.run(
            [self.binary, "--session", self.session, *args],
            capture_output=True,
            text=True,
            timeout=30,
        )

    def ready(self, timeout=30.0):
        deadline = time.time() + timeout
        while time.time() < deadline:
            probe = self.cli("pane", "list")
            if probe.returncode == 0 and "pane_id" in probe.stdout:
                # The server answers before the client in this PTY is ready to forward keys.
                self.pump(2.5)
                return True
            time.sleep(0.5)
        return False

    def visible_text(self):
        pane = self.focused_pane()
        if not pane:
            return ""
        return self.cli("pane", "read", pane, "--source", "visible").stdout

    def wait_for_pane_text(self, token, timeout=20.0):
        """Wait until the pane's visible text contains `token` (the shell echoed the command)."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            if token in self.visible_text():
                return True
            self.pump(0.4)
        return False

    def wait_until_stable(self, timeout=20.0, quiet=0.6):
        """Wait until the pane's visible text stops changing (the fixture finished printing)."""
        deadline = time.time() + timeout
        previous = None
        while time.time() < deadline:
            current = self.visible_text()
            if current and current == previous:
                return True
            previous = current
            self.pump(quiet)
        return False

    def wait_for_log(self, token, timeout=10.0):
        """Wait until the plugin's own state log gains `token`."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            if token in plugin_log():
                return True
            self.pump(0.25)
        return False

    def focused_pane(self):
        listing = self.cli("pane", "list").stdout
        found = re.findall(r'"pane_id":\s*"([^"]+)"', listing)
        return found[0] if found else None

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
        try:
            subprocess.run(
                [self.binary, "session", "delete", self.session],
                capture_output=True,
                timeout=20,
            )
        except Exception:
            pass


def plugin_log():
    for entry in os.listdir(STATE_DIR):
        if "herdr-flash" in entry:
            path = os.path.join(STATE_DIR, entry, "herdr-flash.log")
            if os.path.exists(path):
                return open(path).read()
    return ""


def clipboard():
    return subprocess.run(["pbpaste"], capture_output=True, text=True).stdout


def set_clipboard(text):
    subprocess.run(["pbcopy"], input=text, text=True)


def type_fixture(lab):
    if not lab.wait_for_pane_text("$", timeout=10.0):
        return False
    lab.send("printf '%s' '" + FIXTURE.replace("\n", "\\n") + "'\r", settle=0.6)
    # Wait for the shell to echo the command, then for the output to settle: the capture must see
    # the finished fixture, not a half-drawn prompt.
    if not lab.wait_for_pane_text(EXPECTED):
        return False
    return lab.wait_until_stable()


def pick_and_yank(lab, failures, label):
    """Type a query, pick the hint, then yank with Herdr's own copy mode.

    The status line is redrawn cell by cell, so its text is not reliably contiguous in the client
    stream: the jump is asserted on the plugin's own state log instead.
    """
    lab.send(QUERY, settle=0.8)
    if not lab.wait_for_text("copy mode", timeout=8.0):
        failures.append(f"{label}: the picker did not announce the copy-mode handoff")
        return False
    marker = len(plugin_log())
    lab.send("a", settle=0.5)
    if not lab.wait_for_log("jump=delivered", timeout=10.0):
        failures.append(f"{label}: the plugin never delivered a jump")
        return False
    if "jump=delivered" not in plugin_log()[marker:]:
        failures.append(f"{label}: the jump was not new")
        return False
    # The popup closes when the plugin exits; Herdr applies the jump on the next frame.
    lab.pump(1.2)
    lab.send("v", settle=0.4)
    lab.send("e", settle=0.4)
    lab.send("y", settle=1.2)
    return True


def main():
    binary = os.environ.get("HERDR_BIN") or (PATCHED if os.path.exists(PATCHED) else "herdr")
    stable = os.environ.get("HERDR_STABLE_BIN", "herdr")
    if shutil.which(binary) is None and not os.path.exists(binary):
        print(f"herdr binary not found: {binary}", file=sys.stderr)
        return 2

    saved_clipboard = clipboard()
    failures = []
    lab = Lab(binary, LAB)
    try:
        if not lab.ready():
            failures.append("the lab session never became reachable")
            raise SystemExit(report(failures, lab))
        if not type_fixture(lab):
            failures.append("A: the fixture never appeared in the pane")

        # A. prefix+shift+s from a terminal pane.
        set_clipboard("")
        lab.send("\x01S", settle=1.0)
        if not lab.wait_for_text("herdr-flash"):
            failures.append("A: the flash popup never opened")
        pick_and_yank(lab, failures, "A")
        copied = clipboard()
        if copied != EXPECTED:
            failures.append(f"A: copy mode did not yank the target (clipboard={copied!r})")

        # B. shift+s inside Herdr's copy mode. Let the yank's copy-mode exit and scroll restore
        # settle first: the picker must open from a fresh copy-mode state.
        lab.pump(2.0)
        set_clipboard("")
        lab.send("\x01[", settle=1.2)  # prefix+[ enters copy mode
        lab.send("S", settle=1.0)  # configured copy_mode_key opens flash
        if not lab.wait_for_text("herdr-flash"):
            failures.append("B: copy-mode key never opened the flash popup")
        pick_and_yank(lab, failures, "B")
        copied = clipboard()
        if copied != EXPECTED:
            failures.append(f"B: copy mode did not yank the target (clipboard={copied!r})")

    finally:
        code = report(failures, lab)
        lab.close()

    # C. A Herdr without pane.copy_mode_jump must keep the picker's own copy cursor.
    stable_failures = []
    stable_lab = Lab(stable, LAB_STABLE)
    try:
        if not stable_lab.ready():
            stable_failures.append("C: the stable lab session never became reachable")
        else:
            type_fixture(stable_lab)
            marker = len(plugin_log())
            stable_lab.send("\x01S", settle=1.0)
            if not stable_lab.wait_for_text("herdr-flash", timeout=12.0):
                stable_failures.append("C: the picker never opened on the stock build")
            stable_lab.send(QUERY, settle=0.8)
            if not stable_lab.wait_for_text("enter jump", timeout=8.0):
                stable_failures.append("C: the picker did not keep its own copy cursor")
            stable_lab.send("a", settle=0.8)
            if not stable_lab.wait_for_any(["v select", "y copy", "esc exit"]):
                stable_failures.append("C: the pick did not enter the picker's own copy mode")
            if "handoff=false" not in plugin_log()[marker:]:
                stable_failures.append("C: the stock build was not detected as unsupported")
            if "jump=delivered" in plugin_log()[marker:]:
                stable_failures.append("C: the stock build tried to hand off")
            stable_lab.send("\x1b", settle=0.5)
    finally:
        if stable_failures:
            print("=== C FAILURES ===")
            for failure in stable_failures:
                print("-", failure)
            tail = bytes(stable_lab.output[-2000:]).decode("utf-8", "replace")
            print(repr(tail[-1200:]))
            code = 1
        stable_lab.close()

    set_clipboard(saved_clipboard)
    return code


def report(failures, lab):
    print("=== plugin state log (tail) ===")
    print("\n".join(plugin_log().splitlines()[-8:]))
    if failures:
        print("=== FAILURES ===")
        for failure in failures:
            print("-", failure)
        tail = bytes(lab.output[-4000:]).decode("utf-8", "replace")
        print("=== client stream tail ===")
        print(repr(tail[-2000:]))
        print("=== rendered screen (tail) ===")
        print(lab.plain()[-1500:])
        return 1
    print("=== E2E OK: flash hint -> Herdr copy mode -> Herdr yank ===")
    return 0


if __name__ == "__main__":
    sys.exit(main())
