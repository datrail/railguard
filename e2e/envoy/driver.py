"""Sends a request through each Envoy, and asserts what reached the upstream."""

import json
import sys
import urllib.error
import urllib.request
import uuid

_UPSTREAM = "upstream.test:8080"
_BODY = b'{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
_MARKER = "x-e2e-id"

# Each Envoy, and the status a request through it should get.
_CASES = [
    # The extension answers OK, so the request reaches the upstream, which answers 200.
    ("envoy-ext-authz", 200),
    # The extension answers CONTINUE to the request headers, so the request reaches the upstream.
    ("envoy-ext-proc", 200),
    # The callout fails and failure_mode_allow is off, so ext_authz denies with its default 403.
    ("envoy-no-ext-authz", 403),
    # The callout fails and failure_mode_allow is off, so ext_proc fails the request with 500.
    ("envoy-no-ext-proc", 500),
]


def _send_through(envoy):
    # The agent's request: plain HTTP to the listener, with the target in `Host`.
    marker = str(uuid.uuid4())
    request = urllib.request.Request(
        f"http://{envoy}:15001/mcp",
        data=_BODY,
        headers={"Host": _UPSTREAM, "Content-Type": "application/json", _MARKER: marker},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status, marker
    except urllib.error.HTTPError as error:
        return error.code, marker


def _count_received(marker):
    with urllib.request.urlopen(f"http://{_UPSTREAM}/__admin/requests", timeout=10) as response:
        journal = json.load(response)
    # WireMock keeps some header names as sent and capitalises others.
    return sum(
        1
        for entry in journal["requests"]
        if any(
            name.lower() == _MARKER and value == marker
            for name, value in entry["request"]["headers"].items()
        )
    )


def main():
    passed = True
    for envoy, expected in _CASES:
        status, marker = _send_through(envoy)
        received = _count_received(marker)
        ok = status == expected and received == (1 if expected == 200 else 0)
        passed = passed and ok
        print(f"{'ok  ' if ok else 'FAIL'} {envoy}: {status}, {received} received (expected {expected})")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
