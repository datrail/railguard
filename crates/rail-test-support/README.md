# rail-test-support

Helpers the workspace's tests share, so every crate tests its settings and its binary the same way. A dev-dependency
only: nothing shipped uses it. See the [main README](../../README.md) for the system around it.

## Entry points

- **`read`**, **`read_from`** and **`describe`** read settings as a binary reads them: what's read, what's logged, or
  every refusal.
- **`protect`** reads a `RAIL_PROXY_PROTECTED_HOSTS` value that must be accepted.
- **`isolate`** runs a binary with an empty environment, so the caller's own `RAIL_…` variables can't change a run;
  **`Server`** spawns it as a server, waits until it's serving, or until it has logged a line, and kills it if the test
  ends first; **`wait_until`** polls a condition for up to 10 s; **`stdout`** reads what a run printed.
- **`RailCenter`** stubs Rail Center over HTTP: a bundle, an empty key set, a capture plan or a 204, and 202 for
  everything else; it keeps each request line, and each POST's line and body.

## What it doesn't do

No stubs of a host's other neighbours, such as Envoy or Apigee: each crate's tests keep their own, since what they
answer is the test.
