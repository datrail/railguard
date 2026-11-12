"""Checks rail-envoy's gateway behind Envoy, through each way Envoy calls it: ext_proc full duplex
(as GCP's load balancer does), ext_proc buffered and ext_authz with the body; then what only Envoy
shows: the body sent back whole, the cap, a body mode refused, and a gateway that never answers.

The cases every host passes alike are in gateway_cases.py.
"""

import json

from gateway_cases import (
    AGENT,
    CALL,
    CALLOUT_FAILED,
    GOOD_TICKET,
    TOO_LARGE,
    UPLOAD_TIMEOUT_SECONDS,
    check_every_request_matched,
    count_upstream_requests,
    exit_with_result,
    expect,
    read_last_received_body,
    report_fail,
    report_ok,
    reset_journals,
    run_gateway_cases,
    send_raw_post,
    start_block,
)

_ENVOY = "http://envoy"
_JUDGING = {
    "full duplex": f"{_ENVOY}:10000/mcp",
    "buffered": f"{_ENVOY}:10001/mcp",
    "ext_authz": f"{_ENVOY}:10002/mcp",
}
_STREAMED = f"{_ENVOY}:10003/mcp"
_NO_GATEWAY = f"{_ENVOY}:10004/mcp"


# A `tools/call` padded to about `size` bytes.
def _build_padded_call(size):
    payload = {**CALL, "params": {"name": "track_package", "arguments": {"pad": "x" * size}}}
    return json.dumps(payload).encode()


def _run_envoy_cases():
    good = [("x-rail", GOOD_TICKET), *AGENT.items()]

    for name, mcp in _JUDGING.items():
        start_block(f"{name}: a 3 MiB call reaches the upstream byte for byte")
        reset_journals()
        body = _build_padded_call(3 * 1024 * 1024)
        code, answer = send_raw_post(mcp, body, good, UPLOAD_TIMEOUT_SECONDS)
        if code == 200 and '"text":"delivered"' in answer:
            report_ok("the call returns the upstream's answer")
        else:
            report_fail(f"the call returns the upstream's answer: got {code} {answer[:300]}")
        expect("the upstream received the body as sent", True, read_last_received_body() == body)

        start_block(f"{name}: a 5 MiB call is past RAIL_MAX_BODY_MB")
        reset_journals()
        body = _build_padded_call(5 * 1024 * 1024)
        code, answer = send_raw_post(mcp, body, good, UPLOAD_TIMEOUT_SECONDS)
        expect("the call is answered 413", 413, code)
        expect("with the refusal's body", TOO_LARGE, answer)
        expect("nothing reached the upstream", 0, count_upstream_requests())

    call = json.dumps(CALL).encode()

    start_block("streamed: a body Envoy streams is refused")
    reset_journals()
    code, answer = send_raw_post(_STREAMED, call, good)
    expect("the call is answered 503", 503, code)
    expect("with the refusal's body", CALLOUT_FAILED, answer)
    expect("nothing reached the upstream", 0, count_upstream_requests())

    start_block("no gateway: Envoy fails closed")
    reset_journals()
    code, answer = send_raw_post(_NO_GATEWAY, call, good)
    if code is not None and code >= 500:
        report_ok(f"the call is refused ({code})")
    else:
        report_fail(f"the call is refused with a 5xx: got {code} {answer[:300]}")
    expect("nothing reached the upstream", 0, count_upstream_requests())


def _main():
    for name, mcp in _JUDGING.items():
        print(f"\n# through {name}")
        run_gateway_cases(mcp)
    _run_envoy_cases()
    check_every_request_matched()
    exit_with_result()


if __name__ == "__main__":
    _main()
