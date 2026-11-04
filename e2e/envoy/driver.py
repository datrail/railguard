"""Sends a request through each Envoy, and asserts what reached the upstream."""

import json
import sys
import time
import urllib.error
import urllib.request
import uuid

_UPSTREAM = "upstream.test:8080"
_BODY = b'{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
_MARKER = "x-e2e-id"
_RAIL_CENTER = "rail-center:8080"
_HOST_ID = "host-e2e"
# The agent's own x-rail headers, which the extension must keep from the upstream.
_FORGED = {"x-rail": "forged", "x-rail-status": "forged", "x-rail-other": "forged"}
# The first fetch of each extension's ticket is in flight as the stack comes up.
_SETTLE_SECONDS = 15

# Each Envoy, the sandbox its extension is set up with, whether it fetches a ticket, the status a
# request through it should get, and the x-rail headers the upstream should see.
_CASES = [
    # Rail Center serves a ticket, so it rides in x-rail; the extension allows the request.
    ("envoy-ext-authz", "e2e-sandbox", True, 200, {"x-rail": "e2e-opaque-token"}),
    # The same through ext_proc, which sets the headers in its answer to the request's headers.
    ("envoy-ext-proc", "e2e-sandbox", True, 200, {"x-rail": "e2e-opaque-token"}),
    # Rail Center answers with no ticket for the sandbox, so x-rail-status says so.
    ("envoy-not-found", "not-a-registered-sandbox", True, 200, {"x-rail-status": "not-found"}),
    # Rail Center serves a ticket already past expires_at, which is never presented.
    ("envoy-expired", "e2e-expired", True, 200, {"x-rail-status": "expired"}),
    # Rail Center fails the fetch, and no ticket was ever held.
    ("envoy-issuer-down", "e2e-issuer-down", True, 200, {"x-rail-status": "issuer-unreachable"}),
    # Rail Center is set up but disabled: never reached, and neither header.
    ("envoy-disabled", "e2e-disabled", False, 200, {}),
    # The callout fails and failure_mode_allow is off, so ext_authz denies with its default 403.
    ("envoy-no-ext-authz", None, False, 403, None),
    # The callout fails and failure_mode_allow is off, so ext_proc fails the request with 500.
    ("envoy-no-ext-proc", None, False, 500, None),
]


def _send_through(envoy):
    # The agent's request: plain HTTP to the listener, with the target in `Host`.
    marker = str(uuid.uuid4())
    headers = {"Host": _UPSTREAM, "Content-Type": "application/json", _MARKER: marker, **_FORGED}
    request = urllib.request.Request(
        f"http://{envoy}:15001/mcp", data=_BODY, headers=headers, method="POST"
    )
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status, marker
    except urllib.error.HTTPError as error:
        return error.code, marker


def _find_received(marker):
    """The x-rail headers of each request the upstream got with `marker`."""
    with urllib.request.urlopen(f"http://{_UPSTREAM}/__admin/requests", timeout=10) as response:
        journal = json.load(response)
    received = []
    for entry in journal["requests"]:
        # WireMock keeps some header names as sent and capitalises others.
        headers = {name.lower(): value for name, value in entry["request"]["headers"].items()}
        if headers.get(_MARKER) == marker:
            received.append({name: value for name, value in headers.items() if name.startswith("x-rail")})
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


def _run_case(envoy, expected_status, expected_headers):
    expected_received = [] if expected_headers is None else [expected_headers]
    deadline = time.monotonic() + _SETTLE_SECONDS
    while True:
        status, marker = _send_through(envoy)
        received = _find_received(marker)
        if (status, received) == (expected_status, expected_received) or time.monotonic() > deadline:
            return status == expected_status and received == expected_received, status, received
        time.sleep(1)


def main():
    passed = True
    for envoy, sandbox, fetches_ticket, expected_status, expected_headers in _CASES:
        # Each state is the answer to a real fetch, not the state before the first one.
        if fetches_ticket:
            _wait_for_fetch(sandbox)
        ok, status, received = _run_case(envoy, expected_status, expected_headers)
        fetches = None if sandbox is None else _count_fetches(sandbox)
        ok = ok and (fetches != 0 if fetches_ticket else not fetches)
        passed = passed and ok
        print(f"{'ok  ' if ok else 'FAIL'} {envoy}: {status}, received {received}, {fetches} fetches")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
