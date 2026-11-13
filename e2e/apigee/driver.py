"""Checks rail-apigee on its own, then through the stand-in for an Apigee proxy running apiproxy/;
or, with E2E_TARGET=live (live/session.sh test), through real Apigee, followed by what Apigee is
seen to do where the stand-in can't say.

Assertions read WireMock's request journals, not logs: a log says what rail-apigee believes it
did, a journal says it happened. "Seen live" is what runs on real Apigee found.
"""

import gzip
import http.client
import json
import os

import grpc
from external_callout_pb2 import MessageContext, Request, Strings
from external_callout_pb2_grpc import ExternalCalloutServiceStub
from gateway_cases import (
    AGENT,
    CALL,
    CALLOUT_FAILED,
    GOOD_TICKET,
    P2,
    REFUSED,
    TOO_LARGE,
    UPLOAD_TIMEOUT_SECONDS,
    call_tool,
    check_every_request_matched,
    clear_journals,
    count_bundle_fetches,
    count_denials,
    count_denials_naming,
    count_denials_naming_no_policy,
    count_upstream_requests,
    exit_with_result,
    expect,
    get_call_status,
    read_last_received_body,
    report_fail,
    report_ok,
    report_seen,
    reset_journals,
    run_gateway_cases,
    send_raw_post,
    start_block,
    wait_for,
    wait_for_last_denial,
)
from grpc_health.v1 import health_pb2, health_pb2_grpc

_LIVE = os.environ.get("E2E_TARGET", "local") == "live"
_CALLOUT = "callout:8080"
_UNREACHABLE = "callout-unreachable:9100"
_MCP = os.environ.get("E2E_MCP_URL", "http://apigee:8080/mcp")
_NO_CALLOUT = os.environ.get("E2E_NO_CALLOUT_URL", "http://apigee:8080/no-callout")
_READY = "rail.Ready"
_OPERATOR_FAULT = '{"error": "e2e operator fault"}'
# Apigee's own fault for a body past its limit.
_TOO_BIG = "protocol.http.TooBigBody"


def _check_health(target, service=""):
    """The status's name, or the gRPC error's code."""
    with grpc.insecure_channel(target) as channel:
        stub = health_pb2_grpc.HealthStub(channel)
        try:
            answer = stub.Check(health_pb2.HealthCheckRequest(service=service), timeout=5)
        except grpc.RpcError as error:
            return error.code().name
    return health_pb2.HealthCheckResponse.ServingStatus.Name(answer.status)


def _process(target, verb, content="", uri="/mcp", ticket=None):
    """The flow variables rail-apigee set."""
    headers = {} if ticket is None else {"x-rail": Strings(strings=[ticket])}
    context = MessageContext(request=Request(verb=verb, uri=uri, content=content, headers=headers))
    with grpc.insecure_channel(target) as channel:
        answer = ExternalCalloutServiceStub(channel).ProcessMessage(context, timeout=5)
    return {name: value.string for name, value in answer.additional_flow_variables.items()}


# rail-apigee on its own: its health, its port, and a call judged.
def _run_callout_cases():
    start_block("enrolled: holds the bundle and judges a call")
    expect("a bundle was fetched at startup", 1, wait_for(count_bundle_fetches, 1))
    expect("liveness", "SERVING", _check_health(_CALLOUT))
    expect("readiness", "SERVING", _check_health(_CALLOUT, _READY))
    expect("a GET is allowed", "allow", _process(_CALLOUT, "GET").get("rail.decision"))
    refused = _process(_CALLOUT, "POST", json.dumps(CALL))
    expect("a call with no ticket is refused", "refuse", refused.get("rail.decision"))
    expect("with 403", "403", refused.get("rail.status"))
    expect("and the refusal's body", REFUSED, refused.get("rail.body"))
    expect(
        "the denial is reported, naming no policy",
        1,
        wait_for(count_denials_naming_no_policy, 1),
    )

    forbidden = json.dumps({**CALL, "params": {"name": "forbidden_tool"}})
    decoded = _process(_CALLOUT, "POST", forbidden, "/m%63p", GOOD_TICKET)
    expect(
        "a percent-encoded path is decoded, so P2 denies it",
        "403",
        decoded.get("rail.status"),
    )
    broken = json.dumps({**CALL, "params": {"name": "broken_tool"}})
    broken = _process(_CALLOUT, "POST", broken, ticket=GOOD_TICKET)
    expect("a policy it can't apply is refused", "refuse", broken.get("rail.decision"))
    expect("with 503", "503", broken.get("rail.status"))
    expect("and the refusal's body", CALLOUT_FAILED, broken.get("rail.body"))
    # Through Apigee, its answer failing there would look the same: only asked directly can
    # the refusal be told apart.
    large = _process(_CALLOUT, "POST", "x" * 5 * 1024 * 1024, ticket=GOOD_TICKET)
    expect(
        "a request whose answer Apigee couldn't take is refused",
        "refuse",
        large.get("rail.decision"),
    )
    expect("with 413", "413", large.get("rail.status"))
    expect("and the refusal's body", TOO_LARGE, large.get("rail.body"))

    start_block("unreachable: serves on the port it was given, and isn't ready")
    expect("liveness on 9100", "SERVING", _check_health(_UNREACHABLE))
    expect("readiness on 9100", "NOT_SERVING", _check_health(_UNREACHABLE, _READY))


# Through the stand-in: what Apigee does with rail-apigee's answers.
def _run_apigee_cases():
    good = {"x-rail": GOOD_TICKET, **AGENT}

    start_block("mcp: a ticket Apigee splits on its comma is undecodable")
    reset_journals()
    split = {**good, "x-rail": GOOD_TICKET + ",x"}
    expect("the call is refused 403", 403, get_call_status(_MCP, "track_package", split))
    expect(
        "the denial reads the ticket as undecodable",
        "undecodable",
        wait_for_last_denial().get("metadata", {}).get("x-rail-status"),
    )

    # Seen live: Apigee splits the claim on its comma, so it reads as two and is dropped. No
    # real claim has a comma.
    start_block("mcp: a claimed status with a comma is dropped")
    reset_journals()
    claimed = {"x-rail-status": "not-found, expired", **AGENT}
    expect("the call is refused 403", 403, get_call_status(_MCP, "track_package", claimed))
    metadata = wait_for_last_denial().get("metadata", {})
    expect("the denial reads no ticket", "absent", metadata.get("x-rail-status"))
    expect(
        "and records no claim",
        "<none>",
        metadata.get("claimed-x-rail-status", "<none>"),
    )

    # Seen live: Apigee takes a callout answer of at most 4 MiB, and the answer echoes the body,
    # so rail-apigee refuses a larger call; Apigee answers one past 10 MB itself.
    start_block("mcp: a 3.5 MiB call is forwarded, a 5 MiB one refused, an 11 MiB one too big")
    reset_journals()
    code, body = call_tool(_MCP, "track_package", {"pad": "x" * 7 * 512 * 1024}, good)
    if code == 200 and '"text":"delivered"' in body:
        report_ok("the 3.5 MiB call returns the upstream's answer")
    else:
        report_fail(f"the 3.5 MiB call returns the upstream's answer: got {code} {body[:300]}")
    reset_journals()
    code, body = call_tool(_MCP, "track_package", {"pad": "x" * 5 * 1024 * 1024}, good)
    expect("the 5 MiB call is answered 413", 413, code)
    expect("with the refusal's body", TOO_LARGE, body)
    expect("nothing reached the upstream", 0, count_upstream_requests())
    reset_journals()
    arguments = {"pad": "x" * 11 * 1024 * 1024}
    payload = {**CALL, "params": {"name": "track_package", "arguments": arguments}}
    body = json.dumps(payload).encode()
    code, body = send_raw_post(_MCP, body, list(good.items()), UPLOAD_TIMEOUT_SECONDS)
    expect("the 11 MiB call is answered 413", 413, code)
    expect("by Apigee, as too big", True, _TOO_BIG in body)
    expect("nothing reached the upstream", 0, count_upstream_requests())

    start_block("no-callout: the proxy fails closed")
    reset_journals()
    code, body = call_tool(_NO_CALLOUT, "track_package", headers=good)
    expect("the call is answered 503", 503, code)
    expect("with RF-CalloutFailed's body", CALLOUT_FAILED, body)
    expect("nothing reached the upstream", 0, count_upstream_requests())

    # The FaultRule names EC-Rail's fault, so an operator's policy failing before it
    # (RF-E2E-OperatorFault, test only) keeps its own error.
    start_block("an operator's fault before the callout keeps its own error")
    reset_journals()
    fault = {"x-e2e-operator-fault": "1", **AGENT}
    for name, url in (("mcp", _MCP), ("no-callout", _NO_CALLOUT)):
        code, body = call_tool(url, "track_package", headers=fault)
        expect(f"{name}: the call is answered 418", 418, code)
        expect(f"{name}: with the operator's body", _OPERATOR_FAULT, body)
    expect("nothing reached the upstream", 0, count_upstream_requests())
    # A refused call after it: once its denial is in, the first would be too.
    expect(
        "a call with no ticket is refused",
        403,
        get_call_status(_MCP, "track_package", AGENT),
    )
    expect(
        "only that call reached rail-apigee",
        1,
        wait_for(count_denials_naming_no_policy, 1),
    )
    # Given time for a second report to arrive, were there one.
    expect("and it alone was reported", 1, wait_for(count_denials, 2, tries=8))


# Live only: what Apigee hands the callout, which the callout echoes, so the upstream receives
# what the callout read. Each prints what was seen, for the stand-in and rail-apigee to follow.
def _run_live_observations():
    good = [("x-rail", GOOD_TICKET), *AGENT.items()]

    start_block("seen: what Apigee sends the callout for content that isn't UTF-8")
    clear_journals()
    arguments = b'{"note": "a\xffb"}'
    body = b'{"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": '
    body += b'{"name": "track_package", "arguments": ' + arguments + b"}}"
    code, answer = send_raw_post(_MCP, body, good)
    report_seen("the answer", f"{code} {answer[:200]}")
    received = read_last_received_body()
    marker = b'"note": "'
    if received is None:
        report_seen("the upstream received", "nothing")
    elif marker not in received:
        report_seen("the upstream received, without the note", received[:200])
    else:
        # From the note's value on: a, then U+FFFD (ef bf bd), nothing, or ff, then b.
        note = received[received.index(marker) + len(marker) :][:6]
        report_seen("the upstream received the note as", note.hex(" "))

    start_block("seen: whether Apigee decompresses a body before the callout")
    clear_journals()
    forbidden = json.dumps({**CALL, "params": {"name": "forbidden_tool"}}).encode()
    code, answer = send_raw_post(
        _MCP, gzip.compress(forbidden), [("Content-Encoding", "gzip"), *good]
    )
    report_seen("the answer", f"{code} {answer[:200]}")
    # P2 names the tool, so only a decompressed body can match it.
    named_p2 = wait_for(lambda: count_denials_naming(P2), 1)
    report_seen("P2 denied it, so it was decompressed", "yes" if named_p2 else "no")

    # Its answer is just under 4 MiB: forwarded only if Apigee takes the full 4 MiB.
    start_block("seen: a call just under rail-apigee's cap")
    clear_journals()
    arguments = {"pad": "x" * (4 * 1024 * 1024 - 8 * 1024)}
    payload = {**CALL, "params": {"name": "track_package", "arguments": arguments}}
    body = json.dumps(payload).encode()
    code, answer = send_raw_post(_MCP, body, good, UPLOAD_TIMEOUT_SECONDS)
    report_seen("the answer", f"{code} {answer[:200]}")


def _main():
    # First: the stand-in's cases reset the journals, and these count a fetch at startup.
    if not _LIVE:
        _run_callout_cases()
    run_gateway_cases(_MCP)
    _run_apigee_cases()
    check_every_request_matched()

    if _LIVE:
        try:
            _run_live_observations()
        except (OSError, http.client.HTTPException) as error:
            report_seen("the observations stopped", error)

    exit_with_result()


if __name__ == "__main__":
    _main()
