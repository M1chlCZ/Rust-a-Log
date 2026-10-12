# Log protocol, version 1

The transport is TCP. The default endpoint is 127.0.0.1:7331.
Non-loopback receivers require a token. Direct TCP is plaintext; use a trusted LAN or an SSH tunnel.
The encoding is UTF-8 JSON with one complete frame per line.
Each frame, including the newline, has a 64 KiB limit.

Each new connection sends this registration first:

```json
{"type":"register","version":1,"source":"payments-api","language":"go","token":"receiver-token"}
```

Registration must arrive within three seconds. It occurs once per connection.
The token must match the receiver token. Omit it only when the receiver has no token.
The receiver obtains its token from `RUAL_TOKEN` or `--token-env NAME`.
The token contains 1 to 256 UTF-8 bytes without control characters.
The source name contains 1 to 128 UTF-8 bytes. The language contains 1 to 32 UTF-8 bytes.
Neither value can contain control characters. Each connection gets a separate receiver session ID.
The receiver remembers disconnected sources until its 128-source limit fills.

Then send log records:

```json
{"type":"log","timestamp":"2026-10-12T12:00:00Z","level":"ERROR","message":"Request failed\nstack trace","fields":{"request_id":42}}
```

Message is required and must be a string.
For omitted values, the receiver supplies an arrival timestamp, INFO, and empty fields.
Timestamp is a display string of at most 128 UTF-8 bytes. Fields must be a JSON object.
JSON string escaping keeps a multiline message or stack trace inside one frame.

Supported levels are TRACE, DEBUG, INFO, WARNING, ERROR, and SUCCESS.
The receiver accepts lowercase levels. It maps WARN to WARNING, and FATAL or CRITICAL to ERROR.
Unknown levels, malformed JSON, or oversized frames close that client. Other sources continue.

For discarded records, send:

```json
{"type":"dropped","count":12}
```

Count is a nonnegative integer. It adds to the discarded count for this session.
Send each increment once. The receiver sends no acknowledgment.

The receiver retains at most 2,000 records or 8 MiB of serialized record data.
This is a payload limit, not an exact process memory limit. JSON objects, source metadata, and TUI snapshots require additional memory.
It accepts at most 32 simultaneous socket connections, including clients that did not register yet.

With `rual listen --json`, output has this form:

```json
{"sequence":1,"session":1,"source":"payments-api","language":"go","timestamp":"2026-10-12T12:00:00Z","level":"ERROR","message":"Request failed\nstack trace","fields":{"request_id":42}}
```

Sequence gives receiver order. Application timestamps do not guarantee a global order between processes.
Reconnects create new sessions. The same source name can occur on several sessions.
There is no disk replay. Queues, disconnects, crashes, and expired history can discard records.

## Remote viewers (version 0.1.1 and later)

A viewer sends this instead of registration:

```json
{"type":"subscribe","version":1,"token":"receiver-token"}
```

After authentication, the receiver sends `{"type":"ready","version":1}`.
An authentication failure closes the socket without history or source data.
The receiver then sends status and event frames:

```json
{"type":"status","sources":[{"id":1,"name":"api","language":"go","connected":true,"records":1,"dropped":0}],"evicted":0,"rejected":0,"adapter_dropped":0,"last_error":""}
{"type":"event","event":{"sequence":1,"session":1,"source":"api","language":"go","timestamp":"2026-10-12T12:00:00Z","level":"INFO","message":"Ready","fields":{}}}
```

The initial stream contains retained history, then new records. Status arrives about every 100 ms.
Sequence gaps show history that expired before a viewer read it. A reconnect replays retained history.
Response frames allow 384 KiB plus 1 KiB for metadata and JSON escaping of physical file lines.
Publisher input keeps the 64 KiB limit.
The 32-connection limit includes publishers and viewers. A viewer gets no source ID.
Slow viewers can lose expired records or disconnect when the socket write deadline expires.
No per-source access rules, encrypted TCP, browser transport, or disk replay are provided.
