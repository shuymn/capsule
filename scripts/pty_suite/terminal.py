"""Spawn interactive zsh sessions in real PTYs (standard library only)."""

from __future__ import annotations

import fcntl
import os
from pathlib import Path
import pty
import shutil
import struct
import sys
import termios


def utf8_locale() -> str:
    """Return a UTF-8 locale available by default on the host OS."""
    return "en_US.UTF-8" if sys.platform == "darwin" else "C.UTF-8"


def zsh_env(home: Path, path: str, **extra: str) -> dict[str, str]:
    """Minimal environment for an interactive zsh reading `.zshrc` from `home`."""
    return {
        "HOME": str(home), "ZDOTDIR": str(home), "PATH": path,
        "TERM": "xterm-256color", "LC_ALL": utf8_locale(), **extra,
    }


def spawn_zsh(cwd: Path, env: dict[str, str]) -> tuple[int, int]:
    """Fork an interactive zsh (`-d -i`) on a new PTY; return its pid and master fd."""
    zsh = shutil.which("zsh")
    if zsh is None:
        raise RuntimeError("zsh is required")
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(cwd)
        os.execve(zsh, [zsh, "-d", "-i"], env)
    return pid, fd


def set_window_size(fd: int, rows: int, cols: int) -> None:
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
