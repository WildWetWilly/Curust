#!/usr/bin/env python3
"""Fake network printers (OctoPrint, Moonraker, PrusaLink, Duet) for testing Curust's uploads.

usage: fake_net.py KIND PORT OUTDIR [API_KEY]
Stores uploaded files in OUTDIR and logs every request to OUTDIR/requests.log.
"""
import http.server, json, os, re, sys, urllib.parse

KIND, PORT, OUT = sys.argv[1], int(sys.argv[2]), sys.argv[3]
KEY = sys.argv[4] if len(sys.argv) > 4 else ""
os.makedirs(OUT, exist_ok=True)
state = {"printing": False, "paused": False, "file": "", "progress": 0.0}

def parse_multipart(handler, body):
    ctype = handler.headers.get("Content-Type", "")
    m = re.search(r"boundary=(.+)", ctype)
    parts = {}
    if not m:
        return parts
    boundary = m.group(1).encode()
    for chunk in body.split(b"--" + boundary):
        if b"\r\n\r\n" not in chunk:
            continue
        head, data = chunk.split(b"\r\n\r\n", 1)
        data = data[:-2] if data.endswith(b"\r\n") else data
        nm = re.search(rb'name="([^"]+)"', head)
        fn = re.search(rb'filename="([^"]+)"', head)
        if nm:
            parts[nm.group(1).decode()] = (fn.group(1).decode() if fn else None, data)
    return parts

class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def reply(self, code, obj=None):
        body = json.dumps(obj if obj is not None else {}).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def auth(self):
        if KEY and KIND != "duet" and self.headers.get("X-Api-Key") != KEY:
            self.reply(403, {"error": "Invalid API key"})
            return False
        return True
    def handle_any(self, method):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n) if n else b""
        u = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(u.query)
        with open(os.path.join(OUT, "requests.log"), "a") as f:
            f.write(f"{method} {self.path} {len(body)} {dict(self.headers).get('Content-Type','')}\n")
        if not self.auth():
            return
        p = u.path
        if KIND == "octoprint":
            if p == "/api/version": return self.reply(200, {"api": "0.1", "server": "1.10.0", "text": "OctoPrint 1.10.0 (fake)"})
            if p == "/api/files/local" and method == "POST":
                parts = parse_multipart(self, body)
                fn, data = parts["file"]
                open(os.path.join(OUT, fn), "wb").write(data)
                if parts.get("print", (None, b"false"))[1] == b"true":
                    state.update(printing=True, file=fn)
                return self.reply(201, {"done": True})
            if p == "/api/printer": return self.reply(200, {"state": {"text": "Printing" if state["printing"] else "Operational"}, "temperature": {"tool0": {"actual": 210.1, "target": 210.0}, "bed": {"actual": 60.0, "target": 60.0}}})
            if p == "/api/job" and method == "GET": return self.reply(200, {"state": ("Paused" if state["paused"] else "Printing") if state["printing"] else "Operational", "job": {"file": {"name": state["file"]}}, "progress": {"completion": 12.5, "printTime": 60, "printTimeLeft": 420}})
            if p == "/api/job" and method == "POST":
                c = json.loads(body)
                if c["command"] == "cancel": state.update(printing=False)
                elif c["command"] == "pause": state.update(paused=c.get("action") == "pause")
                return self.reply(204)
            if p == "/api/printer/command": return self.reply(204)
        if KIND == "moonraker":
            if p == "/server/info": return self.reply(200, {"result": {"klippy_state": "ready", "moonraker_version": "v0.9.3 (fake)"}})
            if p == "/server/files/upload":
                parts = parse_multipart(self, body)
                fn, data = parts["file"]
                open(os.path.join(OUT, fn), "wb").write(data)
                if parts.get("print", (None, b"false"))[1] == b"true":
                    state.update(printing=True, file=fn)
                return self.reply(201, {"result": {"item": {"path": fn, "root": "gcodes"}, "print_started": True, "action": "create_file"}})
            if p == "/printer/objects/query": return self.reply(200, {"result": {"status": {"print_stats": {"state": "printing" if state["printing"] else "standby", "filename": state["file"], "print_duration": 30.0}, "virtual_sdcard": {"progress": 0.25}, "extruder": {"temperature": 215.0, "target": 215.0}, "heater_bed": {"temperature": 60.0, "target": 60.0}}}})
            if p.startswith("/printer/print/"):
                a = p.rsplit("/", 1)[1]
                if a == "cancel": state.update(printing=False)
                return self.reply(200, {"result": "ok"})
            if p == "/printer/gcode/script": return self.reply(200, {"result": "ok"})
        if KIND == "prusalink":
            if p == "/api/version": return self.reply(200, {"api": "2.0.0", "server": "2.1.2", "text": "PrusaLink", "hostname": "fake-mk4"})
            if p == "/api/v1/storage": return self.reply(200, {"storage_list": [{"path": "/usb", "available": True, "read_only": False}]})
            m = re.match(r"/api/v1/files/(\w+)/(.+)", p)
            if m and method == "PUT":
                fn = urllib.parse.unquote(m.group(2))
                open(os.path.join(OUT, fn), "wb").write(body)
                if self.headers.get("Print-After-Upload") == "?1":
                    state.update(printing=True, file=fn)
                return self.reply(201, {})
            if p == "/api/v1/status": return self.reply(200, {"printer": {"state": "PRINTING" if state["printing"] else "IDLE", "temp_nozzle": 214.9, "target_nozzle": 215, "temp_bed": 60, "target_bed": 60}, "job": {"id": 7, "progress": 33, "time_printing": 100, "time_remaining": 200} if state["printing"] else None})
            if p.startswith("/api/v1/job/"):
                if method == "DELETE": state.update(printing=False)
                return self.reply(204)
        if KIND == "duet":
            if p == "/rr_connect": return self.reply(200, {"err": 0 if q.get("password", [""])[0] == KEY else 1})
            if p == "/rr_upload":
                name = q["name"][0].split("/")[-1]
                open(os.path.join(OUT, name), "wb").write(body)
                return self.reply(200, {"err": 0})
            if p == "/rr_gcode":
                g = q["gcode"][0]
                with open(os.path.join(OUT, "gcode.log"), "a") as f: f.write(g + "\n")
                if g.startswith("M32"): state.update(printing=True, file=g.split('"')[1])
                return self.reply(200, {"buff": 255})
            if p == "/rr_model":
                k = q.get("key", [""])[0]
                if k == "state.status": return self.reply(200, {"key": k, "result": "processing" if state["printing"] else "idle"})
                if k == "heat": return self.reply(200, {"key": k, "result": {"bedHeaters": [0], "heaters": [{"current": 60, "active": 60}, {"current": 200, "active": 200}]}})
                if k == "job": return self.reply(200, {"key": k, "result": {"file": {"fileName": state["file"], "size": 1000}, "filePosition": 500, "duration": 50, "timesLeft": {"file": 50}}})
                if k == "boards": return self.reply(200, {"key": k, "result": [{"name": "Duet 3 MB6HC (fake)"}]})
        self.reply(404, {"error": "not found: " + p})
    def do_GET(self): self.handle_any("GET")
    def do_POST(self): self.handle_any("POST")
    def do_PUT(self): self.handle_any("PUT")
    def do_DELETE(self): self.handle_any("DELETE")

http.server.ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
