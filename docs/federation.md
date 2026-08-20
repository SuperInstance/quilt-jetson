# Federation — Cross-Tier Quilt Communication

> Status: v0.1.0
> Pair with: [`architecture.md`](architecture.md).

The Quilt ecosystem is a federation of runtimes on different
hardware tiers, all reading the same YAML sheet format and all
talking to each other over the `quilt://` URI scheme.

This document shows how to:

1. **Subscribe** to a remote cell from a Jetson and expose it
   as a local cell.
2. **Expose** a local cell so a remote tier (e.g. a Codespace)
   can subscribe.
3. **Compose** cells across tiers — a formula on a Jetson
   referencing a sensor on an ESP32, with a listener on a
   Codespace firing on the formula.

## The URI scheme

The scheme is `quilt://instance/sheet#cell`:

```
quilt://<instance>/<sheet>#<cell>
```

- `<instance>` is a logical name for a Quilt runtime. It maps to
  a concrete HTTP base URL via the `FederationClient`'s
  instance registry.
- `<sheet>` is the sheet id on that instance.
- `<cell>` is the cell id within that sheet.

Examples:

```
quilt://jetson-lab/perception#vision.obstacles
quilt://esp32-fleet/boat#sensor.heading
quilt://cloud-fleet/audit#autopilot.last_decision
quilt://codespace-1/boat-autopilot#algo.pid_kp
```

## Subscribing from a Jetson

A `sensor` cell with `source: "quilt://..."` is the canonical
way to subscribe to a remote cell:

```yaml
- id: remote.heading
  kind: sensor
  source: quilt://esp32-fleet/boat#sensor.heading
  rate: 10
  default: 0
```

The engine's `FederationClient`:

1. Resolves `esp32-fleet` to an HTTP base URL (e.g.
   `https://boat-1.local:8080`).
2. Opens a WebSocket to the remote instance.
3. Sends a `subscribe` message with the cell URI.
4. Receives value updates and pushes them into the local
   `remote.heading` cell.
5. Downstream formulas and listeners re-evaluate as if the
   data came from a local sensor.

## Registering a remote instance

Before the federation client can resolve an instance name,
the instance must be registered:

```rust
use quilt_jetson::federation::{FederationClient, QuiltRef};

let client = FederationClient::new();
client.register_instance(
    QuiltRef::new("esp32-fleet", "http://boat-1.local:8080")
        .with_token("optional-auth-token"),
);
```

In the CLI binary, this is configured via a config file or
command-line flag. See the `serve` subcommand:

```bash
quilt-jetson serve examples/federation-subscribe.yaml \
    --federation-config /etc/quilt/fleet.yaml
```

The fleet config file has the shape:

```yaml
instances:
  - name: esp32-fleet
    base_url: http://boat-1.local:8080
  - name: cloud-fleet
    base_url: https://fleet.example.com
    auth_token: ${QUILT_CLOUD_TOKEN}
```

## A complete example

This example shows a Jetson subscribed to a remote ESP32's
heading and GPS, with a formula computing a heading error and
a listener that fires when the boat is off course.

```yaml
id: federation-subscribe
title: Subscribe to a remote ESP32 cell
version: 0.1.0

cells:
  # The local proxy for the remote ESP32 cell.
  - id: remote.heading
    kind: sensor
    source: quilt://esp32-fleet/boat#sensor.heading
    rate: 10
    default: 0
    description: |
      Live heading from the ESP32 boat. The federation client
      resolves the URI and subscribes via WebSocket.

  # The desired heading — set locally (e.g. by the autopilot).
  - id: desired.heading
    kind: value
    value: 180
    unit: degrees

  # The heading error — what we want to be zero.
  - id: heading.error
    kind: formula
    expr: "=((desired.heading - remote.heading + 540) % 360) - 180"
    unit: degrees
    description: "Signed shortest-path error."

  # A listener that fires when the boat is off course.
  - id: alert.off_course
    kind: listener
    watch: [heading.error]
    condition: "abs(heading.error) > 30"
    action: log.off_course

  - id: log.off_course
    kind: program
    code: |
      const e = await runtime.get("heading.error");
      return {
        severity: "warn",
        message: `Off course by ${e.data}°`,
        timestamp: Date.now(),
      };
```

When you `quilt-jetson serve` this sheet:

1. The engine loads the sheet.
2. The `FederationClient` resolves `esp32-fleet` to its HTTP
   base URL.
3. A WebSocket connection is opened to the ESP32.
4. The ESP32 sends heading updates; the engine pushes them
   into `remote.heading`.
5. The `heading.error` formula re-evaluates.
6. When `abs(heading.error) > 30`, the `alert.off_course`
   listener fires and `log.off_course` runs.

The same sheet, with a different `quilt://` URI in the
`source` field, would subscribe to a different cell on a
different tier. No code changes needed.

## Exposing a local cell

For a remote tier to subscribe to a cell on a Jetson, the
Jetson's web server (on `:8080`) must be reachable. The
remote tier opens a WebSocket to
`ws://jetson:8080/ws?cell=...` and receives updates.

The web server's WebSocket handler (`GET /ws`) accepts a
`cell` query parameter and subscribes to changes for that
cell. Updates are sent as JSON-encoded `SubscriptionEvent`s.

You don't need to do anything special to "expose" a cell —
just start the web server. The default port is 8080; override
with `--port`.

## Federation semantics

### Updates are at-most-once

Updates from a remote cell are sent over a single WebSocket.
If the connection drops, the client reconnects and re-fetches
the current value. The federation is **eventually consistent**
— the local cell may be briefly out of date after a reconnect.

### Cells are not versioned

There's no version vector or CRDT. The federation is a
"last-writer-wins" model. For cells where that matters
(e.g. counters, accumulators), use a `program` cell with
explicit state.

### Read-only by default

A `quilt://` subscription is read-only: the remote cell's
value is mirrored locally; writes to the local cell are NOT
forwarded back. To make the federation bidirectional, use two
`quilt://` URIs (one inbound, one outbound) with the same
remote cell.

## What's on the wire

The federation client speaks a tiny JSON protocol over
WebSocket:

```json
// Client → server
{ "kind": "subscribe", "uri": "quilt://esp32-fleet/boat#sensor.heading" }

// Server → client (one per change)
{ "kind": "value", "uri": "...", "value": { "data": 270, "status": "ready", ... } }
```

The HTTP fallback (one-shot read) is just
`GET /api/cell/<sheet>/<cell>`.

## Limitations and roadmap

In v0.1.0:

- The federation client is read-only (subscribe to remote
  cells; not yet push local cells to remote subscribers
  outside the WebSocket).
- The HTTP transport uses plain HTTP; TLS is on the roadmap.
- There's no built-in auth beyond the bearer token. For
  production, terminate TLS at a reverse proxy (nginx,
  caddy) and use that proxy's auth.

In v0.2.0 (planned):

- Bidirectional federation — push local cells to remote
  subscribers.
- Cell-level ACLs.
- Replay support — fetch a cell's full history from a
  remote tier.
