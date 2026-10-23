#!/usr/bin/env python3
"""Run the two focused GTK checks on an isolated, invisible Broadway display."""
import os
from pathlib import Path
import random
import socket
import subprocess
import tempfile
import time

root = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="notes-gtk-") as scratch:
    env = os.environ.copy()
    for key, folder in [("XDG_CONFIG_HOME", "config"), ("XDG_STATE_HOME", "state"),
                        ("XDG_DATA_HOME", "data"), ("XDG_CACHE_HOME", "cache"),
                        ("XDG_RUNTIME_DIR", "runtime")]:
        directory = Path(scratch) / folder
        directory.mkdir(mode=0o700)
        env[key] = str(directory)
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    display = f":{random.randrange(200, 30000)}"
    env.update(GDK_BACKEND="broadway", BROADWAY_DISPLAY=display,
               GSETTINGS_BACKEND="memory", GIO_USE_VFS="local", GSK_RENDERER="cairo",
               GTK_A11Y="none", G_DEBUG="fatal-criticals")
    daemon = subprocess.Popen(["gtk4-broadwayd", "-a", "127.0.0.1", "-p", str(port), display],
                              env=env, stdout=subprocess.DEVNULL)
    try:
        for _ in range(100):
            if daemon.poll() is not None:
                raise RuntimeError("Broadway exited before starting")
            try:
                with socket.create_connection(("127.0.0.1", port), timeout=0.1) as client:
                    client.sendall(b"GET / HTTP/1.0\r\n\r\n")
                    client.recv(128)
                    break
            except OSError:
                time.sleep(0.02)
        profile = ["--release"] if os.environ.get("NOTES_TEST_RELEASE") == "1" else []
        selected = [os.environ["NOTES_GTK_FILTER"]] if os.environ.get("NOTES_GTK_FILTER") else []
        packages = ["-p", os.environ["NOTES_GTK_PACKAGE"]] if os.environ.get("NOTES_GTK_PACKAGE") else ["--workspace"]
        cargo = ["dbus-run-session", "--", "cargo", "test", "--locked", *packages, *profile, *selected, "--"]
        listed = subprocess.run([*cargo, "--ignored", "--list"], cwd=root, env=env, check=True,
                                capture_output=True, text=True)
        tests = [line.rsplit(":", 1)[0].strip()
                 for line in listed.stdout.splitlines() if line.endswith(": test")]
        if not tests:
            raise RuntimeError("no ignored GTK tests found")
        for name in tests:
            subprocess.run([*cargo, "--ignored", "--exact", name, "--test-threads=1", "--nocapture"],
                           cwd=root, env=env, check=True)
    finally:
        daemon.terminate()
        daemon.wait(timeout=5)
