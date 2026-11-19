"""Drive independent local fixture and client processes for resource profiling."""
import asyncio
from contextlib import asynccontextmanager
import json
from statistics import median

WARMUP = 100
CALLS = 4000
ALLOCATION_FIELDS = ("allocations", "allocated_bytes", "reallocations",
                     "reallocated_new_size_bytes")
WIRE_FIELDS = ("request_bytes", "response_bytes")
COUNTER_FIELDS = ("requests", "connections", *WIRE_FIELDS)


@asynccontextmanager
async def child(argv, *, cwd, env, stderr):
    process = await asyncio.create_subprocess_exec(
        *argv, cwd=cwd, env=env, stderr=stderr, stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE, limit=4096,
    )
    try:
        yield process
    finally:
        if process.returncode is None:
            try:
                process.kill()
            except ProcessLookupError:
                pass
        process.stdin.close()
        await asyncio.wait_for(process.wait(), 5)


async def write(process, value):
    process.stdin.write(value)
    await asyncio.wait_for(process.stdin.drain(), 5)


async def read(process, timeout=5):
    line = await asyncio.wait_for(process.stdout.readline(), timeout)
    if not line.endswith(b"\n"):
        raise ValueError("missing complete fixture reply")
    frame = json.loads(line)
    if not isinstance(frame, dict):
        raise ValueError("fixture reply is not an object")
    return frame


async def require_exit(process):
    code = await asyncio.wait_for(process.wait(), 5)
    if code != 0:
        raise ValueError(f"fixture child exited with status {code}")


def counters(values, fields):
    if (values.keys() != set(fields)
            or any(type(values[name]) is not int or values[name] < 0 for name in fields)):
        raise ValueError("invalid fixture counters")
    return values


async def snapshot(server):
    await write(server, b"snapshot\n")
    return counters(await read(server), COUNTER_FIELDS)


async def verify_wire(server_binary, cwd, env, stderr):
    async with child([str(server_binary), "--profile-server"], cwd=cwd, env=env,
                     stderr=stderr) as server:
        port = int((await read(server))["address"].removeprefix("127.0.0.1:"))
        prior = await snapshot(server)
        if any(prior.values()):
            raise ValueError("wire control did not start with an idle fixture")
        reader, writer = await asyncio.wait_for(asyncio.open_connection("127.0.0.1", port), 5)
        request_line = (b"POST /bot0:BENCHMARK/getMe HTTP/1.1\r\nHost: localhost\r\n"
                        b"Content-Type: application/json\r\n")
        requests = [request_line + b"Content-Length: 2\r\n\r\n{}",
                    request_line + b"Transfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n"]
        frames = {}
        try:
            for index, (mode, request) in enumerate(zip(("content-length", "chunked"), requests)):
                writer.write(request)
                await asyncio.wait_for(writer.drain(), 5)
                header = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 5)
                status, *lines = header.rstrip(b"\r\n").split(b"\r\n")
                fields = dict(line.lower().split(b": ", 1) for line in lines)
                body = await asyncio.wait_for(reader.readexactly(int(fields[b"content-length"])), 5)
                if status != b"HTTP/1.1 200 OK" or json.loads(body)["result"]["id"] != 7:
                    raise ValueError("wire control received the wrong HTTP fixture reply")
                current = await snapshot(server)
                observed = difference(current, prior)
                if observed != {"requests": 1, "connections": int(index == 0),
                                "response_bytes": len(header) + len(body),
                                "request_bytes": len(request)}:
                    raise ValueError(f"HTTP socket counters differed from exact {mode} bytes")
                frames[mode] = observed
                prior = current
        finally:
            writer.close()
            await asyncio.wait_for(writer.wait_closed(), 5)
        await write(server, b"stop\n")
        await require_exit(server)
    return frames


def difference(after, before):
    delta = {key: after[key] - before[key] for key in COUNTER_FIELDS}
    return counters(delta, COUNTER_FIELDS)


async def profile(server_binary, client_binary, clients, methods, allocation, cwd, env, stderr, trace):
    fields = ALLOCATION_FIELDS if allocation else ("cpu_ns",)
    results = []
    options = {"cwd": cwd, "env": env, "stderr": stderr}
    async with child([str(server_binary), "--profile-server"], **options) as server:
        address = (await read(server))["address"]
        prior = await snapshot(server)
        if any(prior.values()):
            raise ValueError("fixture was not idle before the run")
        for turn in range(len(clients)):
            order = clients[turn:] + clients[:turn]
            for method in methods:
                for name in order:
                    argv = [str(client_binary), "--profile-client", address, name, method]
                    async with child(argv, **options) as process:
                        if await read(process, timeout=20) != {"phase": "ready", "warmup": WARMUP}:
                            raise ValueError("invalid client warm-up receipt")
                        start = await snapshot(server)
                        warm = difference(start, prior)
                        if warm["requests"] != WARMUP or warm["connections"] != 1:
                            raise ValueError("warm-up must complete on exactly one fresh connection")
                        await write(process, b"g")
                        result = await read(process, timeout=45)
                        if result.keys() != {"requests", "totals"} or result["requests"] != CALLS:
                            raise ValueError("incomplete client measurement")
                        totals = counters(result["totals"], fields)
                        end = await snapshot(server)
                        wire = difference(end, start)
                        if (wire["requests"] != CALLS or wire["connections"] != 0
                                or any(wire[field] <= CALLS for field in WIRE_FIELDS)):
                            raise ValueError("measured requests did not reuse the warmed connection")
                        if method == "sendDocument" and wire["request_bytes"] <= CALLS * 65536:
                            raise ValueError("upload wire counters did not include complete files")
                        await require_exit(process)
                        if await snapshot(server) != end:
                            raise ValueError("HTTP bytes were still in flight after the client reply")
                    prior = end
                    row = {"round": turn + 1, "client": name, "scenario": method, "requests": CALLS,
                           "warmup": warm, "measured_socket": wire, "totals": totals}
                    if allocation and (totals["allocations"] == 0 or totals["allocated_bytes"] == 0):
                        raise ValueError("allocation counter was inactive")
                    results.append(row)
                    trace.write(json.dumps(row) + "\n")
                    trace.flush()
        await write(server, b"stop\n")
        await require_exit(server)
    return results


def summarize(rows, clients, methods, allocation_rows):
    cases = []
    for method in methods:
        for client in clients:
            subset = [row for row in rows if (row["scenario"], row["client"]) == (method, client)]
            instrumented = [row for row in allocation_rows
                            if (row["scenario"], row["client"]) == (method, client)]
            rates = {field: median(row["measured_socket"][field] / CALLS for row in subset)
                     for field in WIRE_FIELDS}
            rates["cpu_ns"] = median(row["totals"]["cpu_ns"] / CALLS for row in subset)
            rates.update({field: median(row["totals"][field] / CALLS for row in instrumented)
                          for field in ALLOCATION_FIELDS})
            cases.append({"scenario": method, "client": client, "median_per_request": rates})
    return cases
