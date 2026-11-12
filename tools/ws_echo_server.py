#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Minimal WebSocket echo server for the `wifi_stream` example (stdlib only).

Every binary frame (16 kHz mono s16le PCM) and every text frame (e.g. the
`ping <micros>` RTT probes) is sent back unchanged, so the board plays back its
own processed microphone signal one round trip later.

    python3 tools/ws_echo_server.py [--host 0.0.0.0] [--port 8765] [--delay-ms 0]

If the optional `zeroconf` package is installed, the server also advertises
`_jarvis._tcp` on mDNS so the board finds it without STREAM_URL.
"""

import argparse
import asyncio
import base64
import hashlib
import socket
import struct
import time

GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


async def read_frame(reader):
    b1, b2 = await reader.readexactly(2)
    opcode = b1 & 0x0F
    masked = b2 & 0x80
    length = b2 & 0x7F
    if length == 126:
        (length,) = struct.unpack("!H", await reader.readexactly(2))
    elif length == 127:
        (length,) = struct.unpack("!Q", await reader.readexactly(8))
    mask = await reader.readexactly(4) if masked else b"\0\0\0\0"
    data = bytearray(await reader.readexactly(length))
    if masked:
        for i in range(length):
            data[i] ^= mask[i % 4]
    return opcode, bytes(data)


def encode_frame(opcode, payload):
    header = bytes([0x80 | opcode])
    n = len(payload)
    if n < 126:
        header += bytes([n])
    elif n < 65536:
        header += bytes([126]) + struct.pack("!H", n)
    else:
        header += bytes([127]) + struct.pack("!Q", n)
    return header + payload


async def handle(reader, writer, delay):
    peer = writer.get_extra_info("peername")
    request = await reader.readuntil(b"\r\n\r\n")
    headers = {}
    for line in request.decode(errors="replace").split("\r\n")[1:]:
        if ":" in line:
            k, v = line.split(":", 1)
            headers[k.strip().lower()] = v.strip()
    key = headers.get("sec-websocket-key")
    if not key:
        writer.close()
        return
    accept = base64.b64encode(hashlib.sha1((key + GUID).encode()).digest()).decode()
    writer.write(
        (
            "HTTP/1.1 101 Switching Protocols\r\n"
            "Upgrade: websocket\r\nConnection: Upgrade\r\n"
            f"Sec-WebSocket-Accept: {accept}\r\n\r\n"
        ).encode()
    )
    await writer.drain()
    print(f"{peer} connected")
    frames, nbytes, t0 = 0, 0, time.monotonic()
    try:
        while True:
            opcode, data = await read_frame(reader)
            if opcode == 0x8:  # close
                writer.write(encode_frame(0x8, data[:2]))
                break
            if opcode == 0x9:  # ping -> pong
                writer.write(encode_frame(0xA, data))
            elif opcode in (0x1, 0x2):  # text / binary: echo
                if delay:
                    await asyncio.sleep(delay)
                writer.write(encode_frame(opcode, data))
                if opcode == 0x2:
                    frames += 1
                    nbytes += len(data)
            await writer.drain()
            if time.monotonic() - t0 >= 5:
                print(f"{peer}: {frames} audio frames, {nbytes / 5 / 1000:.1f} kB/s")
                frames, nbytes, t0 = 0, 0, time.monotonic()
    except (asyncio.IncompleteReadError, ConnectionError):
        pass
    finally:
        print(f"{peer} disconnected")
        writer.close()


def advertise(port):
    try:
        from zeroconf import ServiceInfo, Zeroconf  # optional dependency
    except ImportError:
        print("zeroconf not installed: no mDNS advertisement (use STREAM_URL)")
        return None
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.connect(("8.8.8.8", 80))
    ip = s.getsockname()[0]
    s.close()
    info = ServiceInfo(
        "_jarvis._tcp.local.",
        "ws-echo._jarvis._tcp.local.",
        addresses=[socket.inet_aton(ip)],
        port=port,
        properties={"path": "/"},
    )
    zc = Zeroconf()
    zc.register_service(info)
    print(f"mDNS: advertising _jarvis._tcp on {ip}:{port}")
    return zc


async def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--host", default="0.0.0.0")
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--delay-ms", type=float, default=0.0, help="extra echo delay")
    args = ap.parse_args()
    zc = advertise(args.port)
    server = await asyncio.start_server(
        lambda r, w: handle(r, w, args.delay_ms / 1000.0), args.host, args.port
    )
    print(f"listening on ws://{args.host}:{args.port}/")
    try:
        async with server:
            await server.serve_forever()
    finally:
        if zc:
            zc.close()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
