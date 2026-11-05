"""Sends requests through each Envoy, and asserts what reached each upstream."""

import http.client
import json
import socket
import sys
import time
import urllib.error
import urllib.request
import uuid

# Each upstream's journal: the plain one serves upstream.test and upstream-plain.test, the TLS one
# upstream-tls.test and upstream-agent-port.test (and misresolved.test), the one on 443
# upstream-443.test.
_PLAIN = "upstream:8080"
_TLS = "upstream-tls:8080"
_443 = "upstream-443:8080"
_BODY = b'{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
_MARKER = "x-e2e-id"
_RAIL_CENTER = "rail-center:8080"
_HOST_ID = "host-e2e"
_TICKET = {"x-rail": "e2e-opaque-token"}
# The agent's own x-rail headers, which must never reach an upstream.
_FORGED = {
    "x-rail": "forged",
    "x-rail-status": "forged",
    "x-rail-upstream": "tls",
    "x-rail-other": "forged",
}
# Some upstreams read `_` and `-` alike, so every name with `_` is kept away too.
_UNDERSCORED = {"x_rail": "forged"}
# Not an x-rail header, so it must arrive: the journal would show a forged one left in place.
_NEAR_MISS = {"x-railing": "near-miss"}
# The agent's own credential, which crosses as sent, to a protected host too.
_AGENTS_OWN = {"authorization": "Bearer the-agents-own"}
# What upstream-mappings/mcp.json answers, which must reach the agent as sent.
_ANSWER = {"jsonrpc": "2.0", "id": 1, "result": {}}
# The first fetch of each extension's ticket is in flight as the stack comes up.
_SETTLE_SECONDS = 15

# (Through, as `envoy` or `envoy:port`, the agent's `Host`, its extension's sandbox, whether it
# fetches, the status, where it arrives as (journal, scheme, `Host`) or None, the x-rail and `_`
# headers that arrive)
_CASES = [
    # Straight to the upstream, past every Envoy: the journal shows what a forgery looks like.
    (None, "upstream.test:8080", None, False, 200, (_PLAIN, "http", "upstream.test:8080"), {**_FORGED, **_UNDERSCORED}),
    # Protected with a port: the ticket, over HTTPS, there whatever port the agent sends.
    ("envoy-ext-authz", "upstream-tls.test:8443", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    ("envoy-ext-authz", "upstream-tls.test", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    ("envoy-ext-authz", "upstream-tls.test:9999", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    ("envoy-ext-authz", "upstream-tls.test:80", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    # Protected without one: at the agent's port.
    ("envoy-ext-authz", "upstream-agent-port.test:8443", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-agent-port.test:8443"), _TICKET),
    # Without one, and the agent sends none, or :80: at HTTPS's 443.
    ("envoy-ext-authz", "upstream-443.test", "e2e-sandbox", True, 200, (_443, "https", "upstream-443.test"), _TICKET),
    ("envoy-ext-authz", "upstream-443.test:80", "e2e-sandbox", True, 200, (_443, "https", "upstream-443.test"), _TICKET),
    # Protected as http://: the ticket, over plain HTTP.
    ("envoy-ext-authz", "upstream-plain.test:8080", "e2e-sandbox", True, 200, (_PLAIN, "http", "upstream-plain.test:8080"), _TICKET),
    # Not protected: as the agent sent it, with neither header, and the agent's still removed.
    ("envoy-ext-authz", "upstream.test:8080", "e2e-sandbox", True, 200, (_PLAIN, "http", "upstream.test:8080"), {}),
    # Protected, and doesn't resolve: Envoy fails it, and nothing reaches an upstream.
    ("envoy-ext-authz", "nowhere.test:8443", "e2e-sandbox", True, 503, None, None),
    # Protected, and resolves to a server that serves TLS on 8443 (as upstream-tls.test shows) but
    # can't prove the name: the certificate check refuses it, and the server sees no request.
    ("envoy-ext-authz", "misresolved.test:8443", "e2e-sandbox", True, 503, None, None),
    # The same through ext_proc, which sets the headers in its answer to the request's headers.
    ("envoy-ext-proc", "upstream-tls.test:8443", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    ("envoy-ext-proc", "upstream-tls.test", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    ("envoy-ext-proc", "upstream-tls.test:9999", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    ("envoy-ext-proc", "upstream-agent-port.test:8443", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-agent-port.test:8443"), _TICKET),
    ("envoy-ext-proc", "upstream-443.test", "e2e-sandbox", True, 200, (_443, "https", "upstream-443.test"), _TICKET),
    ("envoy-ext-proc", "upstream-plain.test:8080", "e2e-sandbox", True, 200, (_PLAIN, "http", "upstream-plain.test:8080"), _TICKET),
    ("envoy-ext-proc", "upstream.test:8080", "e2e-sandbox", True, 200, (_PLAIN, "http", "upstream.test:8080"), {}),
    ("envoy-ext-proc", "nowhere.test:8443", "e2e-sandbox", True, 503, None, None),
    ("envoy-ext-proc", "misresolved.test:8443", "e2e-sandbox", True, 503, None, None),
    # Both callouts in one chain: ext_proc, second, leaves the request as ext_authz changed it.
    ("envoy-variants", "upstream-tls.test:8443", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    ("envoy-variants", "upstream-tls.test", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    ("envoy-variants", "upstream-agent-port.test:8443", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-agent-port.test:8443"), _TICKET),
    ("envoy-variants", "upstream-plain.test:8080", "e2e-sandbox", True, 200, (_PLAIN, "http", "upstream-plain.test:8080"), _TICKET),
    # ext_authz with `encode_raw_headers`, the headers in `header_map`, and Envoy removing none of
    # the agent's: rail-envoy's answer alone keeps them from crossing.
    ("envoy-variants:15002", "upstream-tls.test:8443", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    ("envoy-variants:15002", "upstream-plain.test:8080", "e2e-sandbox", True, 200, (_PLAIN, "http", "upstream-plain.test:8080"), _TICKET),
    ("envoy-variants:15002", "upstream.test:8080", "e2e-sandbox", True, 200, (_PLAIN, "http", "upstream.test:8080"), {}),
    # Rail Center answers with no ticket for the sandbox, so x-rail-status says so.
    ("envoy-not-found", "upstream-tls.test", "not-a-registered-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), {"x-rail-status": "not-found"}),
    # Rail Center serves a ticket already past expires_at, which is never presented.
    ("envoy-expired", "upstream-tls.test", "e2e-expired", True, 200, (_TLS, "https", "upstream-tls.test:8443"), {"x-rail-status": "expired"}),
    # Rail Center fails the fetch, and no ticket was ever held.
    ("envoy-issuer-down", "upstream-tls.test", "e2e-issuer-down", True, 200, (_TLS, "https", "upstream-tls.test:8443"), {"x-rail-status": "issuer-unreachable"}),
    # Rail Center is set up but disabled: never reached, neither header, still routed.
    ("envoy-disabled", "upstream-tls.test", "e2e-disabled", False, 200, (_TLS, "https", "upstream-tls.test:8443"), {}),
    # The callout fails and Envoy fails open: forwarded as sent, Envoy having removed the agent's
    # x-rail and `_` headers.
    ("envoy-no-ext-authz", "upstream.test:8080", None, False, 200, (_PLAIN, "http", "upstream.test:8080"), {}),
    ("envoy-no-ext-proc", "upstream.test:8080", None, False, 200, (_PLAIN, "http", "upstream.test:8080"), {}),
    # Nor is a protected host routed over HTTPS, despite the forged x-rail-upstream: plain HTTP to
    # its TLS port fails.
    ("envoy-no-ext-authz", "upstream-tls.test:8443", None, False, 502, None, None),
    ("envoy-no-ext-proc", "upstream-tls.test:8443", None, False, 502, None, None),
]

# rail-envoy answers a full-duplex body part as any other, not sending it back, so Envoy drops the
# answer as spurious: the agent gets a 500, and nothing reaches the upstream. Expected to fail so,
# and only so; a pass fails the run.
_FULL_DUPLEX_UNSUPPORTED = ("rail-envoy doesn't send a full-duplex body back", 500)

# The body, as the agent's chunks.
_PARTS = [b'{"jsonrpc":"2.0",', b'"id":1,', b'"method":"tools/list"}']
# Past 15016's `max_request_bytes` (1024), so ext_authz sees only its start.
_OVERSIZED_PARTS = [b'{"jsonrpc":"2.0","id":1,', b'"method":"tools/list","params":{"pad":"' + b"x" * 1500, b'"}}']

# (What Envoy sends rail-envoy, its listener on envoy-variants, the body's chunks, whether trailers
# follow the body, why it's expected to fail and the status the agent then gets, or None)
_BODY_CASES = [
    ("ext_proc BUFFERED", 15011, _PARTS, False, None),
    ("ext_proc BUFFERED", 15011, _PARTS, True, None),
    ("ext_proc BUFFERED_PARTIAL", 15012, _PARTS, False, None),
    ("ext_proc BUFFERED_PARTIAL", 15012, _PARTS, True, None),
    ("ext_proc STREAMED", 15013, _PARTS, False, None),
    ("ext_proc STREAMED", 15013, _PARTS, True, None),
    ("ext_proc STREAMED, without waiting", 15014, _PARTS, False, None),
    ("ext_proc STREAMED, without waiting", 15014, _PARTS, True, None),
    ("ext_proc FULL_DUPLEX_STREAMED", 15015, _PARTS, False, _FULL_DUPLEX_UNSUPPORTED),
    ("ext_proc FULL_DUPLEX_STREAMED", 15015, _PARTS, True, _FULL_DUPLEX_UNSUPPORTED),
    ("ext_proc BUFFERED, trailers sent to rail-envoy", 15017, _PARTS, True, None),
    ("ext_authz with the body", 15016, _PARTS, False, None),
    ("ext_authz with the body, past max_request_bytes", 15016, _OVERSIZED_PARTS, False, None),
]


def _send_through(via, host):
    # The agent's request: plain HTTP to the listener, with the target in `Host`.
    marker = str(uuid.uuid4())
    headers = {
        "Host": host,
        "Content-Type": "application/json",
        _MARKER: marker,
        **_FORGED,
        **_UNDERSCORED,
        **_NEAR_MISS,
        **_AGENTS_OWN,
    }
    address = host if via is None else via if ":" in via else f"{via}:15001"
    request = urllib.request.Request(f"http://{address}/mcp", data=_BODY, headers=headers, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status, response.read(), marker
    except urllib.error.HTTPError as error:
        return error.code, error.read(), marker


def _is_answer(body):
    """Whether `body` is the upstream's answer, unchanged; not JSON is a no."""
    try:
        return json.loads(body) == _ANSWER
    except ValueError:
        return False


def _read_journal(journal):
    with urllib.request.urlopen(f"http://{journal}/__admin/requests", timeout=10) as response:
        return json.load(response)["requests"]


def _read_headers(entry):
    # WireMock keeps some header names as sent and capitalises others.
    return {name.lower(): value for name, value in entry["request"]["headers"].items()}


def _find_received(marker):
    """Where each request with `marker` arrived, and its x-rail, underscored and own headers."""
    received = []
    for journal in [_PLAIN, _TLS, _443]:
        for entry in _read_journal(journal):
            headers = _read_headers(entry)
            if headers.get(_MARKER) == marker:
                rail = {
                    name: value
                    for name, value in headers.items()
                    if name.startswith("x-rail") or "_" in name or name in _AGENTS_OWN
                }
                received.append(((journal, entry["request"]["scheme"], headers.get("host")), rail))
    return received


def _count_fetches(sandbox):
    """How many ticket fetches Rail Center got for this host and `sandbox`."""
    with urllib.request.urlopen(f"http://{_RAIL_CENTER}/__admin/requests", timeout=10) as response:
        journal = json.load(response)
    return sum(
        1
        for entry in journal["requests"]
        if entry["request"]["url"].startswith("/v1/tickets?")
        and entry["request"]["queryParams"].get("host_id", {}).get("values") == [_HOST_ID]
        and entry["request"]["queryParams"].get("sandbox_name", {}).get("values") == [sandbox]
    )


def _wait_for_fetch(sandbox):
    deadline = time.monotonic() + _SETTLE_SECONDS
    while _count_fetches(sandbox) == 0 and time.monotonic() < deadline:
        time.sleep(0.2)


def _run_case(via, host, expected_status, arrives, expected_headers):
    expected = [] if arrives is None else [(arrives, {**expected_headers, **_NEAR_MISS, **_AGENTS_OWN})]
    deadline = time.monotonic() + _SETTLE_SECONDS
    while True:
        status, body, marker = _send_through(via, host)
        received = _find_received(marker)
        answered = expected_status != 200 or status == 200 and _is_answer(body)
        ok = status == expected_status and received == expected and answered
        if ok or time.monotonic() > deadline:
            return ok, status, received
        time.sleep(1)


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args):
        return None


def _check_redirect():
    """A protected host's redirect reaches the agent as sent, and Envoy doesn't follow it."""
    request = urllib.request.Request(
        "http://envoy-ext-authz:15001/redirect", headers={"Host": "upstream-tls.test:8443"}
    )
    try:
        with urllib.request.build_opener(_NoRedirect).open(request, timeout=10) as response:
            status, location = response.status, response.headers.get("location")
    except urllib.error.HTTPError as error:
        status, location = error.code, error.headers.get("location")
    followed = sum(1 for entry in _read_journal(_PLAIN) if entry["request"]["url"] == "/followed")
    ok = (status, location, followed) == (302, "http://upstream.test:8080/followed", 0)
    print(f"{'ok  ' if ok else 'FAIL'} envoy-ext-authz redirect: {status} to {location}, followed {followed} times")
    return ok


def _upload(port, marker, parts, trailers):
    """`parts` to upstream-plain.test, chunked and a moment apart, then a trailer if asked."""
    with socket.create_connection(("envoy-variants", port), timeout=10) as connection:
        connection.sendall(
            "POST /mcp HTTP/1.1\r\n"
            "Host: upstream-plain.test:8080\r\n"
            "Content-Type: application/json\r\n"
            f"{_MARKER}: {marker}\r\n"
            "Transfer-Encoding: chunked\r\n"
            "\r\n".encode()
        )
        for part in parts:
            time.sleep(0.3)
            connection.sendall(b"%x\r\n%s\r\n" % (len(part), part))
        connection.sendall(b"0\r\nx-e2e-trailer: yes\r\n\r\n" if trailers else b"0\r\n\r\n")
        response = http.client.HTTPResponse(connection)
        response.begin()
        return response.status, response.read()


def _check_body(what, port, parts, trailers, known):
    """A body Envoy sends rail-envoy reaches the upstream whole, with the ticket, and the upstream's
    answer reaches the agent; or, if `known`, it fails only as known."""
    marker = str(uuid.uuid4())
    try:
        status, body = _upload(port, marker, parts, trailers)
    except OSError as error:
        status, body = repr(error), b""
    received = [
        (entry["request"]["body"], _read_headers(entry).get("x-rail"))
        for entry in _read_journal(_PLAIN)
        if _read_headers(entry).get(_MARKER) == marker
    ]
    ok = status == 200 and _is_answer(body) and received == [(b"".join(parts).decode(), _TICKET["x-rail"])]
    with_trailers = ", with trailers" if trailers else ""
    if known is None:
        print(f"{'ok  ' if ok else 'FAIL'} envoy-variants:{port} {what}{with_trailers}: {status}, received {received}")
        return ok
    why, known_status = known
    # Any other failure is a broken listener, not the known gap.
    verdict = "XPASS" if ok else "xfail" if (status, received) == (known_status, []) else "FAIL"
    print(f"{verdict} envoy-variants:{port} {what}{with_trailers} ({why}): {status}, received {received}")
    return verdict == "xfail"


def main():
    passed = True
    for via, host, sandbox, fetches_ticket, expected_status, arrives, expected_headers in _CASES:
        # Each state is the answer to a real fetch, not the state before the first one.
        if fetches_ticket:
            _wait_for_fetch(sandbox)
        ok, status, received = _run_case(via, host, expected_status, arrives, expected_headers)
        fetches = None if sandbox is None else _count_fetches(sandbox)
        ok = ok and (fetches != 0 if fetches_ticket else not fetches)
        passed = passed and ok
        print(f"{'ok  ' if ok else 'FAIL'} {via or 'direct'} {host}: {status}, received {received}, {fetches} fetches")
    passed = _check_redirect() and passed
    for what, port, parts, trailers, known in _BODY_CASES:
        passed = _check_body(what, port, parts, trailers, known) and passed
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
