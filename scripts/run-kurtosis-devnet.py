#!/usr/bin/env python3

import argparse
import selectors
import subprocess
import sys
import time
from pathlib import Path


EXECUTION_MARKERS = (
    "Kurtosis CLI is running in a non interactive terminal.",
    "Container images used in this run:",
    "Printing a message",
    "Adding service with name",
    "Starlark code successfully run.",
)
UPLOAD_MARKER = "Uploading and executing package"
RETRYABLE_ERROR_MARKERS = (
    "error reading from server: EOF",
    "connection reset by peer",
    "Client might have cancelled the stream",
    "error reading server preface",
    "HTTP/1.1 header",
)
RETRYABLE_EXIT_CODE = 75


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--package-dir", required=True)
    parser.add_argument("--args-file", required=True)
    parser.add_argument("--enclave", required=True)
    parser.add_argument("--log", required=True)
    parser.add_argument("--stall-timeout-secs", type=int, default=60)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    log_path = Path(args.log)
    log_path.parent.mkdir(parents=True, exist_ok=True)

    cmd = [
        "kurtosis",
        "run",
        args.package_dir,
        "--args-file",
        args.args_file,
        "--enclave",
        args.enclave,
        "--show-enclave-inspect=false",
        "--image-download=missing",
    ]

    with log_path.open("w", encoding="utf-8") as log_file:
        proc = subprocess.Popen(
            cmd,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )

        if proc.stdout is None:
            raise RuntimeError("failed to capture kurtosis output")

        selector = selectors.DefaultSelector()
        selector.register(proc.stdout, selectors.EVENT_READ)

        upload_started_at = None
        execution_started = False
        retryable_error_seen = False

        while True:
            events = selector.select(timeout=1)
            if events:
                line = proc.stdout.readline()
                if line:
                    sys.stdout.write(line)
                    sys.stdout.flush()
                    log_file.write(line)
                    log_file.flush()
                    if upload_started_at is None and UPLOAD_MARKER in line:
                        upload_started_at = time.monotonic()
                    if any(marker in line for marker in EXECUTION_MARKERS):
                        execution_started = True
                    if any(marker in line for marker in RETRYABLE_ERROR_MARKERS):
                        retryable_error_seen = True
                elif proc.poll() is not None:
                    break
            elif proc.poll() is not None:
                break

            if (
                upload_started_at is not None
                and not execution_started
                and time.monotonic() - upload_started_at > args.stall_timeout_secs
            ):
                message = (
                    "Kurtosis timed out waiting for package execution to start after upload.\n"
                )
                sys.stderr.write(message)
                sys.stderr.flush()
                log_file.write(message)
                log_file.flush()
                retryable_error_seen = True
                proc.terminate()
                try:
                    proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()
                return RETRYABLE_EXIT_CODE

        return_code = proc.wait()
        if return_code != 0 and retryable_error_seen:
            return RETRYABLE_EXIT_CODE
        return return_code


if __name__ == "__main__":
    raise SystemExit(main())
