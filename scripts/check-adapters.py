"""Run each language example against the real rual receiver (standard library only)."""
import json
import os
from pathlib import Path
import queue
import subprocess
import threading

root = Path(__file__).resolve().parents[1]
binary = root / "target" / "debug" / ("rual.exe" if os.name == "nt" else "rual")
server = subprocess.Popen(
    [str(binary), "listen", "--bind", "127.0.0.1:0", "--json"],
    stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    env=dict(os.environ, RUAL_TOKEN="adapter-check-token"),
)
try:
    ready = server.stderr.readline().strip()
    assert ready.startswith("rual listening on "), ready
    address = ready.removeprefix("rual listening on ")
    port = address.rsplit(":", 1)[1]
    records = queue.Queue()

    def receive():
        for line in server.stdout:
            records.put(json.loads(line))

    threading.Thread(target=receive, daemon=True).start()
    environment = dict(os.environ, RUAL_ADDRESS=address, RUAL_PORT=port, RUAL_TOKEN="adapter-check-token")
    examples = [
        ("go", ["go", "run", "./example"], "adapters/go"),
        ("node", ["node", "example.mjs"], "adapters/node"),
        ("dotnet", ["dotnet", "run", "--project", "Example/Example.csproj"], "adapters/dotnet"),
        ("java", [os.environ.get("RUAL_MAVEN", "mvn"), "-q", "compile", "exec:java",
                  "-Dexec.mainClass=io.github.m1chlcz.rual.Example"], "adapters/java"),
        ("python", ["python3", "example.py"], "adapters/python"),
        ("ruby", ["ruby", "example.rb"], "adapters/ruby"),
    ]
    for language, command, directory in examples:
        result = subprocess.run(command, cwd=root / directory, env=environment,
                                capture_output=True, text=True, timeout=90)
        assert result.returncode == 0, result.stdout + result.stderr
        output = [records.get(timeout=5), records.get(timeout=5)]
        assert all(record["language"] == language for record in output), output
        assert [record["level"] for record in output] == ["INFO", "ERROR"], output
        assert "Request failed" in output[1]["message"], output
        print(f"{language}: two records received by rual")
finally:
    server.terminate()
    server.wait(timeout=5)
