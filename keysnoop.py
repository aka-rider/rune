#!/usr/bin/env python3
"""
keysnoop.py — print raw terminal sequences from keystrokes.
Pushes a fresh Kitty keyboard protocol frame (flags 1|8).
Press Escape twice or Ctrl+Q to quit.
"""

import sys
import os
import tty
import termios
import select

KITTY_PUSH   = "\x1b[>9u"   # push new frame: disambiguate + alternate keys
KITTY_POP    = "\x1b[<u"    # pop frame on exit


def read_available(fd: int, timeout: float = 0.1) -> bytes:
    buf = b""
    r, _, _ = select.select([fd], [], [], timeout)
    if not r:
        return buf
    buf += os.read(fd, 1)
    while True:
        r, _, _ = select.select([fd], [], [], 0.02)
        if not r:
            break
        buf += os.read(fd, 256)
    return buf


def escape_repr(raw: bytes) -> str:
    parts = []
    for b in raw:
        if b == 0x1b:
            parts.append("ESC")
        elif b < 0x20:
            parts.append(f"^{chr(b ^ 0x40)}")
        elif b == 0x7f:
            parts.append("DEL")
        elif b > 0x7f:
            parts.append(f"\\x{b:02x}")
        else:
            parts.append(chr(b))
    return "".join(parts)


def format_sequence(raw: bytes) -> str:
    hex_str  = " ".join(f"{b:02x}" for b in raw)
    repr_str = escape_repr(raw)
    try:
        text = raw.decode("utf-8")
        printable = all(0x20 <= ord(c) < 0x7f or ord(c) > 0x9f for c in text)
        text_col = f"text : {text!r}" if printable else ""
    except UnicodeDecodeError:
        text_col = ""

    lines = [f"repr : {repr_str}", f"hex  : {hex_str}"]
    if text_col:
        lines.append(text_col)
    return "\n".join(lines)


def write(s: str) -> None:
    sys.stdout.write(s)
    sys.stdout.flush()


def main() -> None:
    fd = sys.stdin.fileno()
    old_attrs = termios.tcgetattr(fd)

    try:
        tty.setraw(fd)
        os.write(fd, KITTY_PUSH.encode())

        write("\r\n=== keysnoop — kitty push frame (flags 1|8) ===\r\n")
        write("Press keys to inspect. Ctrl+Q or double-ESC to quit.\r\n\r\n")

        last_was_esc = False

        while True:
            raw = read_available(fd, timeout=5.0)
            if not raw:
                last_was_esc = False
                continue

            # Ctrl+Q = 0x11
            if raw == b"\x11":
                break

            # Double ESC to quit (single ESC is ambiguous, let it print first)
            if raw == b"\x1b":
                if last_was_esc:
                    break
                last_was_esc = True
            else:
                last_was_esc = False

            write(f"{format_sequence(raw)}\r\n\r\n")

    finally:
        os.write(fd, KITTY_POP.encode())
        termios.tcsetattr(fd, termios.TCSADRAIN, old_attrs)
        write("\r\nBye.\n")


if __name__ == "__main__":
    main()