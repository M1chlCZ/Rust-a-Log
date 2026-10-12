# Local log protocol, version 1

The transport is TCP on a loopback IP. The default endpoint is 127.0.0.1:7331.
The encoding is UTF-8 JSON with one complete frame per line.
Each frame, including the newline, has a 64 KiB limit.

Each new connection sends this registration first:

```json
{"type":"register","version":1,"source":"payments-api","language":"go"}
```

Registration must arrive within three seconds. It occurs once per connection.
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

The receiver rejects non-loopback bind addresses. It does not authenticate other local users.
Use it only on a trusted local machine. Remote collection and browser transport are not part of this protocol.
