using System.Net;
using System.Net.Sockets;
using System.Text.Json;
using Microsoft.Extensions.Logging;
using Rual;

if (args.Contains("--check")) {
    var server = new TcpListener(IPAddress.Loopback, 0);
    server.Start();
    var port = ((IPEndPoint)server.LocalEndpoint).Port;
    var received = Task.Run(async () => {
        using var client = await server.AcceptTcpClientAsync();
        using var reader = new StreamReader(client.GetStream());
        var header = await reader.ReadLineAsync();
        var record = await reader.ReadLineAsync();
        return (header, record);
    });
    using (var provider = new RualProvider("dotnet-check", port: port)) {
        var logger = provider.CreateLogger("Example");
        using (logger.BeginScope("request scope")) logger.LogError("one\ntwo {RequestId}", 42);
    }
    var (header, record) = await received.WaitAsync(TimeSpan.FromSeconds(5));
    using var registration = JsonDocument.Parse(header!);
    using var entry = JsonDocument.Parse(record!);
    if (registration.RootElement.GetProperty("type").GetString() != "register"
        || entry.RootElement.GetProperty("message").GetString() != "one\ntwo 42"
        || entry.RootElement.GetProperty("fields").GetProperty("RequestId").GetInt32() != 42
        || entry.RootElement.GetProperty("fields").GetProperty("scopes").GetArrayLength() != 1) {
        throw new Exception("Wire format or structured fields failed");
    }
    server.Stop();
    using var offline = new RualProvider("offline", port: port);
    var offlineLogger = offline.CreateLogger("Example");
    var watch = System.Diagnostics.Stopwatch.StartNew();
    for (var i = 0; i < 500; i++) offlineLogger.LogInformation("offline");
    offlineLogger.LogInformation(new string('x', 70000));
    if (watch.Elapsed > TimeSpan.FromSeconds(1) || offline.Dropped == 0) throw new Exception("Logging blocked or dropped count missing");
    Console.WriteLine("dotnet checks passed");
    return;
}

using var sink = new RualProvider("dotnet-example", port: int.Parse(Environment.GetEnvironmentVariable("RUAL_PORT") ?? "7331"));
var log = sink.CreateLogger("Example");
log.LogInformation("Application started on {Port}", 8080);
log.LogError("Request failed: {RequestId}", "demo-42");
