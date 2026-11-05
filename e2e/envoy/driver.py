"""Sends a request through each Envoy, and asserts what reached the upstream."""

import json
import sys
import urllib.error
import urllib.request
import uuid

_UPSTREAM = "upstream.test:8080"
_BODY = b'{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
_MARKER = "x-e2e-id"
# The agent's own x-rail headers, which the extension must keep from the upstream.
_FORGED = {"x-rail": "forged", "x-rail-status": "forged", "x-rail-other": "forged"}

# Each Envoy, the status a request through it should get, and the x-rail headers the upstream
# should see.
_CASES = [
    # The extension answers OK, removing the agent's x-rail headers.
    ("envoy-ext-authz", 200, {}),
    # The extension answers CONTINUE to the request headers, removing the agent's x-rail headers.
    ("envoy-ext-proc", 200, {}),
    # The callout fails and failure_mode_allow is off, so ext_authz denies with its default 403.
    ("envoy-no-ext-authz", 403, None),
    # The callout fails and failure_mode_allow is off, so ext_proc fails the request with 500.
    ("envoy-no-ext-proc", 500, None),
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


def main():
    passed = True
    for envoy, expected_status, expected_headers in _CASES:
        status, marker = _send_through(envoy)
        received = _find_received(marker)
        expected_received = [] if expected_headers is None else [expected_headers]
        ok = status == expected_status and received == expected_received
        passed = passed and ok
        print(f"{'ok  ' if ok else 'FAIL'} {envoy}: {status}, received {received}")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
