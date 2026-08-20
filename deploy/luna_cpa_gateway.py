#!/usr/bin/env python3
"""Luna-only CPA gateway with a persistent token-usage ledger."""

from __future__ import annotations

import argparse
import json
import logging
import sqlite3
import time
import urllib.error
import urllib.request
import uuid
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any


MODEL = "gpt-5.6-luna"
MAX_BODY_BYTES = 8 * 1024 * 1024
FORWARDED_RESPONSE_HEADERS = {
    "content-type",
    "retry-after",
    "x-cpa-trace-id",
    "x-request-id",
}


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds")


def first_int(*values: Any) -> int | None:
    for value in values:
        if isinstance(value, int) and not isinstance(value, bool):
            return value
    return None


def extract_usage(response_body: bytes) -> dict[str, Any]:
    try:
        payload = json.loads(response_body)
    except (UnicodeDecodeError, json.JSONDecodeError):
        return {}

    usage = payload.get("usage") if isinstance(payload, dict) else None
    if not isinstance(usage, dict):
        return {}

    input_details = usage.get("prompt_tokens_details")
    if not isinstance(input_details, dict):
        input_details = usage.get("input_tokens_details")
    if not isinstance(input_details, dict):
        input_details = {}

    output_details = usage.get("completion_tokens_details")
    if not isinstance(output_details, dict):
        output_details = usage.get("output_tokens_details")
    if not isinstance(output_details, dict):
        output_details = {}

    input_tokens = first_int(usage.get("prompt_tokens"), usage.get("input_tokens"))
    output_tokens = first_int(usage.get("completion_tokens"), usage.get("output_tokens"))
    total_tokens = first_int(usage.get("total_tokens"))
    if total_tokens is None and input_tokens is not None and output_tokens is not None:
        total_tokens = input_tokens + output_tokens

    return {
        "input_tokens": input_tokens,
        "cached_input_tokens": first_int(
            input_details.get("cached_tokens"),
            usage.get("prompt_cache_hit_tokens"),
        ),
        "output_tokens": output_tokens,
        "reasoning_tokens": first_int(output_details.get("reasoning_tokens")),
        "total_tokens": total_tokens,
        "usage_json": json.dumps(usage, separators=(",", ":"), sort_keys=True),
    }


class UsageLedger:
    def __init__(self, path: Path) -> None:
        self.path = path
        path.parent.mkdir(parents=True, exist_ok=True)
        with self._connect() as connection:
            connection.execute("PRAGMA journal_mode=WAL")
            connection.execute(
                """
                CREATE TABLE IF NOT EXISTS requests (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    request_id TEXT NOT NULL UNIQUE,
                    upstream_request_id TEXT,
                    started_at TEXT NOT NULL,
                    finished_at TEXT NOT NULL,
                    duration_ms INTEGER NOT NULL,
                    path TEXT NOT NULL,
                    model TEXT NOT NULL,
                    http_status INTEGER NOT NULL,
                    success INTEGER NOT NULL,
                    input_tokens INTEGER,
                    cached_input_tokens INTEGER,
                    output_tokens INTEGER,
                    reasoning_tokens INTEGER,
                    total_tokens INTEGER,
                    error_type TEXT,
                    usage_json TEXT
                )
                """
            )
            connection.execute(
                "CREATE INDEX IF NOT EXISTS requests_started_at_idx ON requests(started_at)"
            )

    def _connect(self) -> sqlite3.Connection:
        connection = sqlite3.connect(self.path, timeout=10)
        connection.execute("PRAGMA busy_timeout=10000")
        return connection

    def record(self, row: dict[str, Any]) -> None:
        columns = (
            "request_id",
            "upstream_request_id",
            "started_at",
            "finished_at",
            "duration_ms",
            "path",
            "model",
            "http_status",
            "success",
            "input_tokens",
            "cached_input_tokens",
            "output_tokens",
            "reasoning_tokens",
            "total_tokens",
            "error_type",
            "usage_json",
        )
        values = tuple(row.get(column) for column in columns)
        placeholders = ",".join("?" for _ in columns)
        with self._connect() as connection:
            connection.execute(
                f"INSERT INTO requests ({','.join(columns)}) VALUES ({placeholders})",
                values,
            )


class GatewayServer(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, address: tuple[str, int], upstream: str, ledger: UsageLedger) -> None:
        self.upstream = upstream.rstrip("/")
        self.ledger = ledger
        super().__init__(address, GatewayHandler)


class GatewayHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "luna-cpa-gateway"
    sys_version = ""

    @property
    def gateway(self) -> GatewayServer:
        return self.server  # type: ignore[return-value]

    def log_message(self, message: str, *args: Any) -> None:
        logging.info("%s %s", self.client_address[0], message % args)

    def do_GET(self) -> None:  # noqa: N802
        if self.path == "/healthz":
            self._send_json(200, {"status": "ok", "model": MODEL})
            return
        if self.path == "/v1/models":
            self._proxy_models()
            return
        self._send_error(404, "not_found", "Route not found")

    def do_POST(self) -> None:  # noqa: N802
        if self.path != "/v1/chat/completions":
            self._send_error(404, "not_found", "Route not found")
            return

        body = self._read_json_body()
        if body is None:
            return
        if body.get("model") != MODEL:
            self._send_error(400, "model_not_allowed", f"Only {MODEL} is allowed")
            return
        if body.get("stream") is True:
            self._send_error(
                400,
                "stream_not_allowed",
                "Streaming is disabled so each request can be fully accounted",
            )
            return

        encoded_body = json.dumps(body, separators=(",", ":")).encode()
        self._proxy_completion(encoded_body)

    def _read_json_body(self) -> dict[str, Any] | None:
        raw_length = self.headers.get("Content-Length")
        try:
            length = int(raw_length or "")
        except ValueError:
            self._send_error(411, "length_required", "Content-Length is required")
            return None
        if length <= 0 or length > MAX_BODY_BYTES:
            self._send_error(413, "request_too_large", "Request body is outside the allowed size")
            return None
        try:
            payload = json.loads(self.rfile.read(length))
        except (UnicodeDecodeError, json.JSONDecodeError):
            self._send_error(400, "invalid_json", "Request body must be valid JSON")
            return None
        if not isinstance(payload, dict):
            self._send_error(400, "invalid_json", "Request body must be a JSON object")
            return None
        return payload

    def _upstream_request(self, path: str, data: bytes | None = None) -> tuple[int, Any, bytes]:
        headers = {"Accept": "application/json"}
        authorization = self.headers.get("Authorization")
        if authorization:
            headers["Authorization"] = authorization
        if data is not None:
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(
            f"{self.gateway.upstream}{path}",
            data=data,
            headers=headers,
            method="POST" if data is not None else "GET",
        )
        try:
            with urllib.request.urlopen(request, timeout=240) as response:
                return response.status, response.headers, response.read()
        except urllib.error.HTTPError as error:
            return error.code, error.headers, error.read()

    def _proxy_models(self) -> None:
        try:
            status, headers, body = self._upstream_request("/v1/models")
        except (OSError, urllib.error.URLError, TimeoutError):
            self._send_error(502, "upstream_unavailable", "CPA upstream is unavailable")
            return
        if status == 200:
            try:
                payload = json.loads(body)
                payload["data"] = [
                    model for model in payload.get("data", []) if model.get("id") == MODEL
                ]
                body = json.dumps(payload, separators=(",", ":")).encode()
            except (AttributeError, UnicodeDecodeError, json.JSONDecodeError):
                self._send_error(502, "invalid_upstream_response", "CPA returned invalid JSON")
                return
        self._relay_response(status, headers, body)

    def _proxy_completion(self, body: bytes) -> None:
        request_id = str(uuid.uuid4())
        started_at = utc_now()
        started = time.monotonic()
        status = 502
        headers: Any = {}
        response_body = b""
        error_type: str | None = None
        try:
            status, headers, response_body = self._upstream_request(
                "/v1/chat/completions", body
            )
        except (OSError, urllib.error.URLError, TimeoutError):
            error_type = "upstream_unavailable"
            response_body = json.dumps(
                {
                    "error": {
                        "type": error_type,
                        "message": "CPA upstream is unavailable",
                    }
                },
                separators=(",", ":"),
            ).encode()

        usage = extract_usage(response_body)
        if status >= 400 and error_type is None:
            try:
                error = json.loads(response_body).get("error", {})
                error_type = str(error.get("type") or error.get("code") or "upstream_error")
            except (AttributeError, UnicodeDecodeError, json.JSONDecodeError):
                error_type = "upstream_error"

        duration_ms = round((time.monotonic() - started) * 1000)
        upstream_request_id = None
        if headers:
            upstream_request_id = headers.get("X-CPA-TRACE-ID") or headers.get("X-Request-ID")
        row = {
            "request_id": request_id,
            "upstream_request_id": upstream_request_id,
            "started_at": started_at,
            "finished_at": utc_now(),
            "duration_ms": duration_ms,
            "path": self.path,
            "model": MODEL,
            "http_status": status,
            "success": int(200 <= status < 300),
            "error_type": error_type,
            **usage,
        }
        try:
            self.gateway.ledger.record(row)
        except sqlite3.Error:
            logging.exception("failed to record usage for request %s", request_id)

        self._relay_response(status, headers, response_body, request_id=request_id)

    def _relay_response(
        self,
        status: int,
        headers: Any,
        body: bytes,
        *,
        request_id: str | None = None,
    ) -> None:
        self.send_response(status)
        if headers:
            for name, value in headers.items():
                if name.lower() in FORWARDED_RESPONSE_HEADERS:
                    self.send_header(name, value)
        if request_id:
            self.send_header("X-Luna-Request-ID", request_id)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_json(self, status: int, payload: dict[str, Any]) -> None:
        body = json.dumps(payload, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_error(self, status: int, error_type: str, message: str) -> None:
        self._send_json(status, {"error": {"type": error_type, "message": message}})


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bind", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8327)
    parser.add_argument("--upstream", default="http://127.0.0.1:8317")
    parser.add_argument("--database", type=Path, required=True)
    args = parser.parse_args()

    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    ledger = UsageLedger(args.database)
    server = GatewayServer((args.bind, args.port), args.upstream, ledger)
    logging.info("Luna CPA gateway listening on %s:%d", args.bind, args.port)
    server.serve_forever()


if __name__ == "__main__":
    main()
