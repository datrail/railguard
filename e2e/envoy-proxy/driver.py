"""Sends requests through each Envoy, and asserts what reached each upstream."""

import http.client
import json
import socket
import sys
import time
import urllib.error
import urllib.parse
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
# fmt: off
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
    # Through ext_proc, which sets the headers in its answer to the request's headers; the port
    # rules above are rail-envoy's own, alike for both callouts.
    ("envoy-ext-proc", "upstream-tls.test:8443", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    ("envoy-ext-proc", "upstream-tls.test", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    ("envoy-ext-proc", "upstream-plain.test:8080", "e2e-sandbox", True, 200, (_PLAIN, "http", "upstream-plain.test:8080"), _TICKET),
    ("envoy-ext-proc", "upstream.test:8080", "e2e-sandbox", True, 200, (_PLAIN, "http", "upstream.test:8080"), {}),
    ("envoy-ext-proc", "nowhere.test:8443", "e2e-sandbox", True, 503, None, None),
    ("envoy-ext-proc", "misresolved.test:8443", "e2e-sandbox", True, 503, None, None),
    # Both callouts in one chain: ext_proc, second, leaves the request as ext_authz changed it.
    ("envoy-variants", "upstream-tls.test:8443", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
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
# fmt: on

# rail-envoy answers a full-duplex body part as any other, not sending it back, so Envoy drops the
# answer as spurious: the agent gets a 500, and nothing reaches the upstream. Expected to fail so,
# and only so; a pass fails the run.
_FULL_DUPLEX_UNSUPPORTED = ("rail-envoy doesn't send a full-duplex body back", 500)

# The body, as the agent's chunks.
_PARTS = [b'{"jsonrpc":"2.0",', b'"id":1,', b'"method":"tools/list"}']

# (What Envoy sends rail-envoy, through, as `envoy:port`, the body's chunks, whether trailers
# follow the body, why it's expected to fail and the status the agent then gets, or None)
_BODY_CASES = [
    ("ext_authz with the body", "envoy-ext-authz:15001", _PARTS, False, None),
    ("ext_proc BUFFERED_PARTIAL", "envoy-ext-proc:15001", _PARTS, False, None),
    (
        "ext_proc BUFFERED_PARTIAL, trailers sent to rail-envoy",
        "envoy-ext-proc:15001",
        _PARTS,
        True,
        None,
    ),
    ("ext_proc BUFFERED", "envoy-variants:15011", _PARTS, False, None),
    ("ext_proc BUFFERED", "envoy-variants:15011", _PARTS, True, None),
    ("ext_proc BUFFERED_PARTIAL", "envoy-variants:15012", _PARTS, False, None),
    ("ext_proc BUFFERED_PARTIAL", "envoy-variants:15012", _PARTS, True, None),
    ("ext_proc STREAMED", "envoy-variants:15013", _PARTS, False, None),
    ("ext_proc STREAMED", "envoy-variants:15013", _PARTS, True, None),
    ("ext_proc STREAMED, without waiting", "envoy-variants:15014", _PARTS, False, None),
    ("ext_proc STREAMED, without waiting", "envoy-variants:15014", _PARTS, True, None),
    (
        "ext_proc FULL_DUPLEX_STREAMED",
        "envoy-variants:15015",
        _PARTS,
        False,
        _FULL_DUPLEX_UNSUPPORTED,
    ),
    (
        "ext_proc FULL_DUPLEX_STREAMED",
        "envoy-variants:15015",
        _PARTS,
        True,
        _FULL_DUPLEX_UNSUPPORTED,
    ),
    ("ext_proc BUFFERED, trailers sent to rail-envoy", "envoy-variants:15017", _PARTS, True, None),
]

# Only its calls can match the capture plan's entry (rail-center's capture-plan.json); not
# protected, so they go to the plain upstream as sent.
_CAPTURED_HOST = "upstream-capture.test:8080"
_MIB = 1024 * 1024
# RAIL_MAX_BODY_MB's default.
_MAX_BODY_BYTES = _MIB
# Capture's windows are a minute long, and sent once closed.
_WINDOW_SECONDS = 60
# Past a window's end, for its document to arrive.
_SEND_SECONDS = 30


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
    request = urllib.request.Request(
        f"http://{address}/mcp", data=_BODY, headers=headers, method="POST"
    )
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
    return sum(
        1
        for entry in _read_journal(_RAIL_CENTER)
        if entry["request"]["url"].startswith("/v1/tickets?")
        and entry["request"]["queryParams"].get("host_id", {}).get("values") == [_HOST_ID]
        and entry["request"]["queryParams"].get("sandbox_name", {}).get("values") == [sandbox]
    )


def _wait_for_fetch(sandbox):
    deadline = time.monotonic() + _SETTLE_SECONDS
    while _count_fetches(sandbox) == 0 and time.monotonic() < deadline:
        time.sleep(0.2)


def _run_case(via, host, expected_status, arrives, expected_headers):
    expected = (
        [] if arrives is None else [(arrives, {**expected_headers, **_NEAR_MISS, **_AGENTS_OWN})]
    )
    deadline = time.monotonic() + _SETTLE_SECONDS
    while True:
        status, body, marker = _send_through(via, host)
        received = _find_received(marker)
        answered = expected_status != 200 or status == 200 and _is_answer(body)
        ok = status == expected_status and received == expected and answered
        if ok or time.monotonic() > deadline:
            return ok, status, received
        time.sleep(1)


def _report(ok, line):
    print(f"{'ok  ' if ok else 'FAIL'} {line}")


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
    _report(ok, f"envoy-ext-authz redirect: {status} to {location}, followed {followed} times")
    return ok


def _upload(via, marker, parts, trailers, host="upstream-plain.test:8080"):
    """`parts` to `host` through `via`, chunked and a moment apart, then a trailer if asked."""
    address, port = via.split(":")
    with socket.create_connection((address, int(port)), timeout=10) as connection:
        connection.sendall(
            "POST /mcp HTTP/1.1\r\n"
            f"Host: {host}\r\n"
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


def _check_body(what, via, parts, trailers, known):
    """A body Envoy sends rail-envoy reaches the upstream whole, with the ticket, and the upstream's
    answer reaches the agent; or, if `known`, it fails only as known."""
    marker = str(uuid.uuid4())
    try:
        status, body = _upload(via, marker, parts, trailers)
    except OSError as error:
        status, body = repr(error), b""
    received = [
        (entry["request"]["body"], _read_headers(entry).get("x-rail"))
        for entry in _read_journal(_PLAIN)
        if _read_headers(entry).get(_MARKER) == marker
    ]
    ok = (
        status == 200
        and _is_answer(body)
        and received == [(b"".join(parts).decode(), _TICKET["x-rail"])]
    )
    with_trailers = ", with trailers" if trailers else ""
    if known is None:
        _report(ok, f"{via} {what}{with_trailers}: {status}, received {received}")
        return ok
    why, known_status = known
    # Any other failure is a broken listener, not the known gap.
    verdict = "XPASS" if ok else "xfail" if (status, received) == (known_status, []) else "FAIL"
    case = f"{via} {what}{with_trailers} ({why})"
    print(f"{verdict} {case}: {status}, received {received}")
    return verdict == "xfail"


def _build_tool_call(repo, content=""):
    """A `create_or_update_file` call to `repo`, which the capture plan's entry takes."""
    arguments = {"repo": repo, "content": content}
    params = {"name": "create_or_update_file", "arguments": arguments}
    return json.dumps(
        {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params}
    ).encode()


# Past the cap, and past Envoy's buffer limit (2 MiB), where both callouts cut it short.
_LARGE = 3 * _MIB


def _split(body):
    """`body` as three chunks."""
    third = len(body) // 3 + 1
    return [body[start : start + third] for start in range(0, len(body), third)]


# (Through, as `envoy:port`, what Envoy sends rail-envoy, the body, whether it's sent chunked, and
# how capture reads it: ("e1", the repository its record is keyed by) or ("unread", why))
# fmt: off
_CAPTURE_CASES = [
    ("envoy-ext-authz:15001", "ext_authz, whole", _build_tool_call("authz-whole"), False, ("e1", "authz-whole")),
    ("envoy-ext-authz:15001", "ext_authz, chunked", _build_tool_call("authz-chunked"), True, ("e1", "authz-chunked")),
    ("envoy-ext-authz:15001", "ext_authz, past the cap", _build_tool_call("authz-past", "x" * _LARGE), False, ("unread", "past_cap")),
    ("envoy-ext-authz:15001", "ext_authz, past the cap, chunked", _build_tool_call("authz-past", "x" * _LARGE), True, ("unread", "past_cap")),
    ("envoy-ext-authz:15001", "ext_authz, not JSON", b"not json {", False, ("unread", "not_json")),
    ("envoy-ext-proc:15001", "ext_proc BUFFERED_PARTIAL, whole", _build_tool_call("proc-whole"), False, ("e1", "proc-whole")),
    ("envoy-ext-proc:15001", "ext_proc BUFFERED_PARTIAL, chunked", _build_tool_call("proc-chunked"), True, ("e1", "proc-chunked")),
    ("envoy-ext-proc:15001", "ext_proc BUFFERED_PARTIAL, past the cap", _build_tool_call("proc-past", "x" * _LARGE), False, ("unread", "past_cap")),
    ("envoy-ext-proc:15001", "ext_proc BUFFERED_PARTIAL, past the cap, chunked", _build_tool_call("proc-past", "x" * _LARGE), True, ("unread", "past_cap")),
    ("envoy-ext-proc:15001", "ext_proc BUFFERED_PARTIAL, not JSON", b"not json {", False, ("unread", "not_json")),
    # Without the body: ext_authz not asked for it, ext_proc's streamed mode not read yet.
    ("envoy-variants:15002", "ext_authz without the body", _build_tool_call("authz-unsent"), False, ("unread", "not_sent")),
    ("envoy-variants:15013", "ext_proc STREAMED", _build_tool_call("proc-unsent"), False, ("unread", "not_sent")),
]
# fmt: on


def _post(via, marker, body):
    """`body` to the capture host through `via`, with its `Content-Length`."""
    headers = {"Host": _CAPTURED_HOST, "Content-Type": "application/json", _MARKER: marker}
    request = urllib.request.Request(f"http://{via}/mcp", data=body, headers=headers, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def _find_by_marker(journal, marker):
    """The requests `journal` got with `marker`, asked for, since its bodies run to MiBs."""
    query = json.dumps({"headers": {_MARKER: {"equalTo": marker}}}).encode()
    request = urllib.request.Request(f"http://{journal}/__admin/requests/find", data=query)
    with urllib.request.urlopen(request, timeout=10) as response:
        return json.load(response)["requests"]


# The recommended configurations, which keep a body's `Content-Length`, as some others don't.
_RECOMMENDED = ["envoy-ext-authz:15001", "envoy-ext-proc:15001"]


def _send_captured(via, what, body, chunked):
    """`body` crosses whole, its `Content-Length` kept by the recommended configurations, and the
    upstream's answer comes back."""
    marker = str(uuid.uuid4())
    try:
        if chunked:
            status, answer = _upload(via, marker, _split(body), False, host=_CAPTURED_HOST)
        else:
            status, answer = _post(via, marker, body)
    except OSError as error:
        status, answer = repr(error), b""
    received = [
        (len(entry["body"]), {name.lower(): value for name, value in entry["headers"].items()})
        for entry in _find_by_marker(_PLAIN, marker)
    ]
    crossed = [(size, headers.get("content-length")) for size, headers in received]
    if via in _RECOMMENDED:
        expected = [(len(body), None if chunked else str(len(body)))]
    else:
        expected = [(len(body), length) for _, length in crossed]
    ok = status == 200 and _is_answer(answer) and crossed == expected
    _report(ok, f"{via} capture, {what}: {status}, received (bytes, length) {crossed}")
    return ok


def _count_plan_fetches():
    """How many capture plan fetches Rail Center got for `ext`'s sandbox."""
    count = 0
    for entry in _read_journal(_RAIL_CENTER):
        url = urllib.parse.urlsplit(entry["request"]["url"])
        sandbox = urllib.parse.parse_qs(url.query).get("sandbox_name")
        count += url.path == "/v1/capture-plan" and sandbox == ["e2e-sandbox"]
    return count


def _read_documents():
    """The call record documents Rail Center got, each once: a retry sends the same one."""
    documents = {}
    for entry in _read_journal(_RAIL_CENTER):
        request = entry["request"]
        if request["method"] == "POST" and request["url"] == "/v1/call-records":
            document = json.loads(request["body"])
            documents[document["document_id"]] = document
    return list(documents.values())


def _read_capture(documents):
    """What `documents` say of the capture host's calls: the entry's records' calls and bytes by
    repository, the catch-all's calls, the host's total, and the bodies unread by why."""
    host = _CAPTURED_HOST.split(":")[0]
    records, catch_all, total, unread = {}, 0, 0, {}
    for document in documents:
        for reason, count in document["truncated"]["body_unread"].items():
            if count:
                unread[reason] = unread.get(reason, 0) + count
        total += sum(hosts["count"] for hosts in document["hosts"] if hosts["host"] == host)
        for record in document["records"]:
            keys, measures = record["context_keys"], record["measures"]
            if record["entry_id"] == "e1":
                calls, size = records.get(keys["destination"], (0, 0))
                records[keys["destination"]] = (
                    calls + measures["call_count"],
                    size + measures["request_bytes"],
                )
            elif keys.get("host") == host:
                catch_all += measures["call_count"]
    return records, catch_all, total, unread


def _check_capture():
    """The capture host's calls through both callouts, in the documents Rail Center gets once
    their windows close: each body read whole within the cap, else counted unread, and why."""
    deadline = time.monotonic() + _SETTLE_SECONDS
    while _count_plan_fetches() == 0 and time.monotonic() < deadline:
        time.sleep(0.2)
    passed = True
    for via, what, body, chunked, _ in _CAPTURE_CASES:
        passed = _send_captured(via, what, body, chunked) and passed
    records, unread = {}, {}
    for _, _, body, _, (kind, value) in _CAPTURE_CASES:
        if kind == "e1":
            records[value] = (1, len(body))
        else:
            unread[value] = unread.get(value, 0) + 1
    expected = (records, sum(unread.values()), len(_CAPTURE_CASES), unread)
    # The last call's window closes at the next boundary, and its document is sent then.
    until = (time.time() // _WINDOW_SECONDS + 1) * _WINDOW_SECONDS + _SEND_SECONDS
    while True:
        read = _read_capture(_read_documents())
        if read == expected or time.time() > until:
            break
        time.sleep(2)
    ok = read == expected
    _report(ok, f"capture's documents: (records, catch-all calls, host total, unread) {read}")
    if not ok:
        print(f"     expected {expected}")
    return passed and ok


def _main():
    passed = True
    for via, host, sandbox, fetches_ticket, expected_status, arrives, expected_headers in _CASES:
        # Each state is the answer to a real fetch, not the state before the first one.
        if fetches_ticket:
            _wait_for_fetch(sandbox)
        ok, status, received = _run_case(via, host, expected_status, arrives, expected_headers)
        fetches = None if sandbox is None else _count_fetches(sandbox)
        ok = ok and (fetches != 0 if fetches_ticket else not fetches)
        passed = passed and ok
        _report(ok, f"{via or 'direct'} {host}: {status}, received {received}, {fetches} fetches")
    passed = _check_redirect() and passed
    for what, via, parts, trailers, known in _BODY_CASES:
        passed = _check_body(what, via, parts, trailers, known) and passed
    passed = _check_capture() and passed
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(_main())
