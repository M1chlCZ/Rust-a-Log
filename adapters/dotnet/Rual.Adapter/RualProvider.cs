using System.Net;
using System.Net.Sockets;
using System.Text.Json;
using System.Threading.Channels;
using Microsoft.Extensions.Logging;

namespace Rual;

/// <summary>An extra, best-effort output for Microsoft.Extensions.Logging.</summary>
public sealed class RualProvider : ILoggerProvider, ISupportExternalScope {
    private readonly Channel<byte[]> queue = Channel.CreateBounded<byte[]>(new BoundedChannelOptions(64) {
        FullMode = BoundedChannelFullMode.Wait, SingleReader = true
    });
    private readonly CancellationTokenSource stop = new();
    private readonly Task worker;
    private readonly IPAddress address;
    private readonly int port;
    private readonly string source;
    private long dropped;
    private int disposed;
    private IExternalScopeProvider scopes = new LoggerExternalScopeProvider();

    public long Dropped => Interlocked.Read(ref dropped);

    public RualProvider(string source, string host = "127.0.0.1", int port = 7331) {
        address = IPAddress.Parse(host);
        if (!IPAddress.IsLoopback(address) || port is < 1 or > 65535) throw new ArgumentException("Use a loopback IP and port");
        if (string.IsNullOrWhiteSpace(source) || System.Text.Encoding.UTF8.GetByteCount(source) > 128) throw new ArgumentException("Invalid source");
        this.source = source;
        this.port = port;
        worker = Task.Run(Send);
    }

    public ILogger CreateLogger(string categoryName) => new Logger(this, categoryName);
    public void SetScopeProvider(IExternalScopeProvider scopeProvider) => scopes = scopeProvider;

    private void Enqueue(object entry) {
        try {
            var bytes = JsonSerializer.SerializeToUtf8Bytes(entry);
            if (bytes.Length + 1 > 65536 || !queue.Writer.TryWrite(bytes)) Interlocked.Increment(ref dropped);
        } catch (Exception) { Interlocked.Increment(ref dropped); }
    }

    private async Task Write(TcpClient client, object entry) =>
        await Write(client, JsonSerializer.SerializeToUtf8Bytes(entry));

    private async Task Write(TcpClient client, byte[] bytes) {
        using var deadline = CancellationTokenSource.CreateLinkedTokenSource(stop.Token);
        deadline.CancelAfter(200);
        var stream = client.GetStream();
        await stream.WriteAsync(bytes, deadline.Token);
        await stream.WriteAsync(new byte[] { 10 }, deadline.Token);
    }

    private async Task Send() {
        TcpClient? client = null;
        long reported = 0;
        var nextConnect = DateTime.MinValue;
        try {
            await foreach (var entry in queue.Reader.ReadAllAsync(stop.Token)) {
                try {
                    if (client is null) {
                        if (DateTime.UtcNow < nextConnect) { Interlocked.Increment(ref dropped); continue; }
                        client = new TcpClient(address.AddressFamily);
                        using var deadline = CancellationTokenSource.CreateLinkedTokenSource(stop.Token);
                        deadline.CancelAfter(100);
                        await client.ConnectAsync(address, port, deadline.Token);
                        await Write(client, new { type = "register", version = 1, source, language = "dotnet" });
                    }
                    var lost = Dropped;
                    if (lost > reported) await Write(client, new { type = "dropped", count = lost - reported });
                    await Write(client, entry);
                    reported = lost;
                } catch (Exception) {
                    Interlocked.Increment(ref dropped);
                    client?.Dispose();
                    client = null;
                    nextConnect = DateTime.UtcNow.AddMilliseconds(200);
                }
            }
        } catch (OperationCanceledException) { }
        finally {
            client?.Dispose();
            while (queue.Reader.TryRead(out _)) Interlocked.Increment(ref dropped);
        }
    }

    public void Dispose() {
        if (Interlocked.Exchange(ref disposed, 1) != 0) return;
        queue.Writer.TryComplete();
        if (!worker.Wait(500)) { stop.Cancel(); worker.Wait(500); }
        stop.Dispose();
    }

    private sealed class Logger(RualProvider owner, string category) : ILogger {
        public IDisposable? BeginScope<TState>(TState state) where TState : notnull => owner.scopes.Push(state);
        public bool IsEnabled(LogLevel logLevel) => logLevel != LogLevel.None;
        public void Log<TState>(LogLevel level, EventId eventId, TState state, Exception? exception, Func<TState, Exception?, string> formatter) {
            if (!IsEnabled(level)) return;
            try {
                var fields = new Dictionary<string, object?> { ["category"] = category, ["event_id"] = eventId.Id };
                if (state is IEnumerable<KeyValuePair<string, object?>> attributes) {
                    foreach (var (key, value) in attributes) if (key != "{OriginalFormat}") fields[key] = value;
                }
                var scopeValues = new List<object?>();
                owner.scopes.ForEachScope((value, target) => target.Add(value), scopeValues);
                if (scopeValues.Count != 0) fields["scopes"] = scopeValues;
                if (exception is not null) fields["exception"] = exception.ToString();
                var name = level switch {
                    LogLevel.Information => "INFO", LogLevel.Warning => "WARNING",
                    LogLevel.Critical => "ERROR", _ => level.ToString().ToUpperInvariant()
                };
                owner.Enqueue(new { type = "log", timestamp = DateTimeOffset.UtcNow.ToString("O"), level = name, message = formatter(state, exception), fields });
            } catch (Exception) { Interlocked.Increment(ref owner.dropped); }
        }
    }
}
