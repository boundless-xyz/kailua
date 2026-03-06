#!/usr/bin/env python3

import argparse
import os
import pty
import sys


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tee")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()

    if not args.command:
        print("usage: pty-run.py [--tee path] <command> [args...]", file=sys.stderr)
        return 2

    log_handle = None
    if args.tee:
        log_handle = open(args.tee, "wb")

    def master_read(fd: int) -> bytes:
        data = os.read(fd, 1024)
        if log_handle and data:
            log_handle.write(data)
            log_handle.flush()
        return data

    try:
        status = pty.spawn(args.command, master_read=master_read)
    finally:
        if log_handle:
            log_handle.close()

    if os.WIFEXITED(status):
        return os.WEXITSTATUS(status)
    if os.WIFSIGNALED(status):
        return 128 + os.WTERMSIG(status)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
