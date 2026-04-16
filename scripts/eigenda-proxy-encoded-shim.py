#!/usr/bin/env python3

import argparse
import sys
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


PAYLOAD_ENCODING_VERSION = 0


def encode_payload(payload: bytes) -> bytes:
    header = bytearray(32)
    header[1] = PAYLOAD_ENCODING_VERSION
    header[2:6] = len(payload).to_bytes(4, "big")

    padded_chunks = []
    for offset in range(0, len(payload), 31):
        chunk = payload[offset : offset + 31]
        padded_chunks.append(b"\x00" + chunk.ljust(31, b"\x00"))

    return bytes(header) + b"".join(padded_chunks)


class EncodedPayloadShimHandler(BaseHTTPRequestHandler):
    upstream_base = ""
    upstream_timeout_seconds = 60

    def do_GET(self) -> None:
        if self.path == "/health" or self.path.startswith("/health?"):
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", "2")
            self.end_headers()
            self.wfile.write(b"ok")
            return

        parsed_path = urllib.parse.urlsplit(self.path)
        query = urllib.parse.parse_qs(parsed_path.query, keep_blank_values=True)
        wants_encoded_payload = (
            query.get("return_encoded_payload", ["false"])[-1].lower() == "true"
        )
        if wants_encoded_payload:
            query.pop("return_encoded_payload", None)

        upstream_query = urllib.parse.urlencode(query, doseq=True)
        upstream_path = urllib.parse.urlunsplit(
            ("", "", parsed_path.path, upstream_query, "")
        )
        upstream_url = urllib.parse.urljoin(
            self.upstream_base.rstrip("/") + "/", upstream_path.lstrip("/")
        )

        request = urllib.request.Request(upstream_url, method="GET")
        try:
            with urllib.request.urlopen(
                request, timeout=self.upstream_timeout_seconds
            ) as response:
                status = response.getcode()
                headers = response.headers
                body = response.read()
        except urllib.error.HTTPError as error:
            status = error.code
            headers = error.headers
            body = error.read()
        except Exception as error:  # pragma: no cover - exercised by integration test
            self.send_error(502, f"Failed to reach upstream EigenDA proxy: {error}")
            return

        if status == 200 and wants_encoded_payload:
            body = encode_payload(body)
            content_type = "application/octet-stream"
        else:
            content_type = headers.get_content_type()

        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt: str, *args) -> None:
        sys.stderr.write(f"[eigenda-proxy-encoded-shim] {fmt % args}\n")


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Compatibility shim that proxies EigenDA GET requests and re-encodes "
            "successful payload responses when return_encoded_payload=true."
        )
    )
    parser.add_argument("--upstream", required=True, help="Base URL of the upstream proxy")
    parser.add_argument(
        "--listen-host", default="127.0.0.1", help="Host interface to bind"
    )
    parser.add_argument(
        "--listen-port", required=True, type=int, help="Port to bind"
    )
    args = parser.parse_args()

    EncodedPayloadShimHandler.upstream_base = args.upstream
    server = ThreadingHTTPServer((args.listen_host, args.listen_port), EncodedPayloadShimHandler)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
