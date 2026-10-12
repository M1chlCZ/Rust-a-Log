# Language adapters

Start `rual tui` or `rual listen --json` before the examples.
The default receiver address is `127.0.0.1:7331`.
The adapters add a best-effort output to the existing logger. Keep original outputs for durable logs.

| Environment | Integration | Example from the repository root |
| --- | --- | --- |
| Go 1.22+ | slog handler | `cd adapters/go && go run ./example` |
| Node.js 20+ / TypeScript | Direct logger, Pino transport | `node adapters/node/example.mjs` |
| .NET 8+ | ILoggerProvider | `dotnet run --project adapters/dotnet/Example` (example needs .NET 10 SDK) |
| Java 17+ | JDK Handler, Logback Appender | `cd adapters/java && mvn compile exec:java -Dexec.mainClass=io.github.m1chlcz.rual.Example` |
| Python 3.10+ | logging.Handler | `python3 adapters/python/example.py` |
| Ruby 3.2+ | Logger output | `ruby adapters/ruby/example.rb` |

For a different receiver, set `RUAL_ADDRESS=127.0.0.1:9000` for the Go example.
Set `RUAL_HOST` and `RUAL_PORT` for the other examples.
These variables configure the examples. Application adapters accept an address or port through their constructors.

## LAN connections

On the receiver, set `RUAL_TOKEN` and run `rual tui --bind 192.168.1.20:7331`.
On the service machine, set the same `RUAL_TOKEN` and the receiver host and port:

```sh
export RUAL_TOKEN='the-same-token-as-the-receiver'
export RUAL_HOST=192.168.1.20
export RUAL_PORT=7331
export RUAL_ADDRESS=192.168.1.20:7331 # Go example
python3 adapters/python/example.py
```

The six example commands use these variables. All adapters read `RUAL_TOKEN` unless you pass a token explicitly.
Remote endpoints require a token. Direct TCP is plaintext; use a trusted LAN or an SSH tunnel.
The .NET adapter accepts an IP address. Other adapters also accept hostnames.

For constructor configuration:

```text
Go:     rual.New("192.168.1.20:7331", "api", token)
Node:   createLogger({ host: "192.168.1.20", port: 7331, source: "api", token })
.NET:   new RualProvider("api", host: "192.168.1.20", port: 7331, token: token)
Java:   new RualClient("api", "192.168.1.20", 7331, token)
Python: RualHandler("api", host="192.168.1.20", token=token)
Ruby:   Rual::LogDevice.new("api", host: "192.168.1.20", token: token)
```

Java Handler and Logback configurations use `RUAL_TOKEN` from the service environment.

## Go

Add the module:

```sh
go get github.com/M1chlCZ/Rust-a-Log/adapters/go@main
```

```go
import (
    "log/slog"
    rual "github.com/M1chlCZ/Rust-a-Log/adapters/go"
)

handler, err := rual.New("127.0.0.1:7331", "worker")
if err != nil { return err }
defer handler.Close()
logger := slog.New(handler)
logger.Error("Request failed", "request_id", 42)
```

The handler preserves slog attributes and groups.
`handler.Dropped()` returns the discarded count. `Close()` waits for a limited queue drain.
Set this logger as the application default with `slog.SetDefault(logger)` only if this output replaces the current default.

## Node.js and TypeScript

Install the source package from a local clone:

```sh
npm install ./path/to/Rust-a-Log/adapters/node
```

```ts
import { createLogger } from "@rust-a-log/node";

const logger = createLogger({ source: "web" });
logger.error("Request failed", { request_id: 42 });
await logger.close();
```

The package includes TypeScript declarations.
`logger.dropped` returns the discarded count. `close()` drains the queue for at most 500 ms.
The package is not published on npm.

For an existing Pino logger:

```js
import pino from "pino";

const transport = pino.transport({
  target: "@rust-a-log/node/pino",
  options: { source: "web" },
});
const logger = pino(transport);
logger.error({ request_id: 42 }, "Request failed");
// On application shutdown:
transport.end();
```

Pino runs the sender in its transport worker. Fields retain the original Pino context.
Browser TypeScript cannot use this Node TCP adapter.

## .NET

Add a project reference:

```sh
dotnet add path/to/YourApp.csproj reference path/to/Rust-a-Log/adapters/dotnet/Rual.Adapter/Rual.Adapter.csproj
```

Register the provider beside the existing providers:

```csharp
using Rual;

builder.Logging.AddProvider(new RualProvider("api"));
```

Existing ILogger calls then forward records.
The provider retains structured state, scopes, category, event ID, and exceptions.
`Dropped` returns the discarded count. The logging host disposes the provider on shutdown.
The library targets .NET 8. The example uses the .NET 10 SDK.
The package is not published on NuGet.

## Java

Build and install the local adapter JAR:

```sh
cd adapters/java
mvn install
```

Then add it to the application:

```xml
<dependency>
  <groupId>io.github.m1chlcz</groupId>
  <artifactId>rual-adapter</artifactId>
  <version>0.1.1-SNAPSHOT</version>
</dependency>
```

These coordinates refer to the local Maven repository. The artifact is not published to Maven Central.

For JDK logging:

```java
import io.github.m1chlcz.rual.RualHandler;

try (var handler = new RualHandler("api", "127.0.0.1", 7331)) {
    var logger = java.util.logging.Logger.getLogger("api");
    logger.addHandler(handler);
    logger.info("Application started");
    // Keep the handler for the lifetime of the application.
    logger.removeHandler(handler);
}
```

For Logback, retain the existing dependency and appenders. Add this inside the existing logback.xml configuration:

```xml
<appender name="RUAL" class="io.github.m1chlcz.rual.RualAppender">
  <source>api</source>
  <host>127.0.0.1</host>
  <port>7331</port>
</appender>
<!-- Add this reference inside the existing root logger: -->
<appender-ref ref="RUAL"/>
```

The Logback adapter retains MDC fields, logger name, thread, and exception stack traces.
`dropped()` returns the discarded count. Shutdown waits for a limited queue drain.

## Python

Copy `adapters/python/rual.py` into your project, or add that directory to `PYTHONPATH`.
The adapter uses the Python standard library.

```python
import logging
from rual import RualHandler

handler = RualHandler("api")
logger = logging.getLogger("api")
logger.addHandler(handler)
logger.error("Request failed", extra={"fields": {"request_id": 42}})
handler.close()  # At application shutdown
logger.removeHandler(handler)
```

The handler retains the logger name, application timestamp, fields, and exception traceback.
Use `logger.exception(...)` inside an exception handler to send its traceback.
`handler.dropped` returns the discarded count. `close()` waits at most one second for the worker.

## Ruby

Copy `adapters/ruby/rual.rb` into your project. The adapter uses the Ruby standard library.

```ruby
require 'logger'
require_relative 'rual'

output = Rual::LogDevice.new('api')
logger = Logger.new(output)
logger.error('Request failed')
output.log('ERROR', 'Request failed', { request_id: 42 })
logger.close # At application shutdown
```

Standard Logger output retains its formatted message and severity.
Use `output.log` for structured fields. `output.dropped` returns the discarded count.
`close` waits at most one second for the worker.

## Delivery

Each queue holds at most 64 records. Each frame, including its newline, has a 64 KiB limit.
Network writes run in the background. A full queue discards new records.
The sender reconnects after a connection failure. Socket failures do not become application errors.
Close or dispose the adapter during application shutdown to drain queued records for a limited time.

Delivery is best effort. TCP does not guarantee that the receiver processed a record before a disconnect.
There is no acknowledgment or disk replay.
The adapter reports discarded counts after a later successful send. Its local counter also works without the receiver.

See [the protocol](../docs/protocol.md) for a custom language adapter.
