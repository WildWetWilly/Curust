#!/usr/bin/env python3
"""A fake Marlin printer on a pseudo-terminal, for testing USB printing.

usage: fake_marlin.py OUT_LOG [--baud-check] [--resend-every N]
Prints the PTY path on stdout, then answers like Marlin: M105 temperatures, M115 firmware name,
"ok" after every command, checksum/line-number validation with Resend requests.
"""
import os, pty, sys, time, tty, re

log_path = sys.argv[1]
resend_every = int(sys.argv[sys.argv.index("--resend-every") + 1]) if "--resend-every" in sys.argv else 0
master, slave = pty.openpty()
tty.setraw(slave)
print(os.ttyname(slave), flush=True)
log = open(log_path, "w")
buf = b""
expected = 0
received = 0
hot, target = 25.0, 0.0
bed, btarget = 24.0, 0.0
def send(s):
    os.write(master, s.encode() + b"\n")
send("start")
send("echo:Marlin 2.1.2 (fake)")
while True:
    try:
        data = os.read(master, 4096)
    except OSError:
        break
    if not data:
        break
    buf += data
    while b"\n" in buf:
        line, buf = buf.split(b"\n", 1)
        line = line.decode(errors="replace").strip()
        if not line:
            continue
        m = re.match(r"N(\d+)(.*)\*(\d+)$", line)
        if m:
            n, body, cs = int(m.group(1)), m.group(2), int(m.group(3))
            calc = 0
            for ch in f"N{n}{body}":
                calc ^= ord(ch)
            if calc != cs:
                send(f"Error:checksum mismatch, Last Line: {expected - 1}"); send(f"Resend: {expected}"); send("ok"); continue
            if body.startswith("M110"):
                expected = n
            if n != expected:
                send(f"Error:Line Number is not Last Line Number+1, Last Line: {expected - 1}"); send(f"Resend: {expected}"); send("ok"); continue
            received += 1
            if resend_every and received % resend_every == 0 and not body.startswith("M110"):
                send(f"Error:checksum mismatch, Last Line: {expected - 1}"); send(f"Resend: {expected}"); send("ok"); continue
            expected = n + 1
            log.write(body + "\n"); log.flush()
            cmd = body
        else:
            cmd = line
            log.write("# " + line + "\n"); log.flush()
        if cmd.startswith("M105"):
            hot += (target - hot) * 0.3; bed += (btarget - bed) * 0.3
            send(f"ok T:{hot:.2f} /{target:.2f} B:{bed:.2f} /{btarget:.2f} @:0 B@:0")
        elif cmd.startswith("M115"):
            send("FIRMWARE_NAME:Marlin 2.1.2 (fake) SOURCE_CODE_URL:github.com/MarlinFirmware/Marlin PROTOCOL_VERSION:1.0 MACHINE_TYPE:Fake EXTRUDER_COUNT:1")
            send("ok")
        else:
            mt = re.search(r"M10[49] .*S(\d+)", cmd)
            if mt: target = float(mt.group(1))
            mb = re.search(r"M1(40|90) .*S(\d+)", cmd)
            if mb: btarget = float(mb.group(2))
            send("ok")
