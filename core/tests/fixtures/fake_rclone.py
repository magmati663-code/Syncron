#!/usr/bin/env python3
"""Atrapa rclone na potrzeby testów QA (SYNCRON_RCLONE).

Każde konto `nazwa:ścieżka` (nazwa >= 2 znaki bez '/') jest mapowane na FAKE_RCLONE_ROOT/ścieżka.
Zmienne środowiskowe:
  FAKE_RCLONE_ROOT   katalog "chmury"
  FAKE_RCLONE_LOG    plik, do którego dopisywane są wywołania (JSON na linię: argv, t0, t1)
  FAKE_RCLONE_CTRL   plik JSON: {"omit_modtime": bool, "rules": [reguły]}
  FAKE_RCLONE_DELAY  stałe opóźnienie każdej operacji (sekundy)
Reguła: {"op": "lsjson"|"*", "match": "podłańcuch argv", "skip": N, "times": M|null,
         "sleep": s, "stderr": "...", "code": k, "stdout": "...", "synth": N}
 - sleep: czekaj, potem (jeśli brak innych pól) wykonaj operację normalnie,
 - stdout (+ opcjonalnie code/stderr): podmień wynik,
 - code/stderr: zakończ błędem,
 - synth: lsjson zwraca N syntetycznych plików d<i>/f<i>.txt.
"""
import datetime
import fcntl
import json
import os
import re
import shutil
import sys
import time

ROOT = os.environ.get("FAKE_RCLONE_ROOT", "")
CTRL = os.environ.get("FAKE_RCLONE_CTRL")
LOG = os.environ.get("FAKE_RCLONE_LOG")
args = sys.argv[1:]
T0 = time.time()
VALUE_FLAGS = {"--bwlimit", "--min-age", "--max-depth", "--contimeout", "--state", "--result"}


def log_call():
    if not LOG:
        return
    line = json.dumps({"argv": args, "t0": T0, "t1": time.time(), "pid": os.getpid()}) + "\n"
    with open(LOG, "a") as f:
        fcntl.flock(f, fcntl.LOCK_EX)
        f.write(line)
        f.flush()
        fcntl.flock(f, fcntl.LOCK_UN)


def die(msg, code):
    sys.stderr.write(msg + "\n")
    log_call()
    sys.exit(code)


def positional():
    out, i = [], 1
    while i < len(args):
        a = args[i]
        if a in VALUE_FLAGS:
            i += 2
            continue
        if not a.startswith("-"):
            out.append(a)
        i += 1
    return out


def flag_value(name):
    for i, a in enumerate(args):
        if a == name and i + 1 < len(args):
            return args[i + 1]
    return None


def resolve(p):
    m = re.match(r"^([^/\\:]{2,}):(.*)$", p, re.S)
    if m:
        rest = m.group(2).lstrip("/")
        return os.path.join(ROOT, rest) if rest else ROOT
    return p


def ctrl():
    if CTRL and os.path.exists(CTRL):
        with open(CTRL) as f:
            return json.load(f)
    return {}


def counter(i):
    path = f"{CTRL}.cnt.{i}"
    with open(path, "a+") as f:
        fcntl.flock(f, fcntl.LOCK_EX)
        f.seek(0)
        n = int(f.read() or "0")
        f.seek(0)
        f.truncate()
        f.write(str(n + 1))
        f.flush()
        fcntl.flock(f, fcntl.LOCK_UN)
    return n


def applicable_rule(op):
    joined = "\x00".join(args)
    for i, r in enumerate(ctrl().get("rules", [])):
        if r.get("op", "*") not in ("*", op):
            continue
        if r.get("match") and r["match"] not in joined:
            continue
        n = counter(i)
        if n < r.get("skip", 0):
            continue
        times = r.get("times")
        if times is not None and n >= r.get("skip", 0) + times:
            continue
        return r
    return None


def mtime_str(st):
    ns = st.st_mtime_ns
    secs, frac = divmod(ns, 10**9)
    d = datetime.datetime.fromtimestamp(secs, datetime.timezone.utc)
    return d.strftime("%Y-%m-%dT%H:%M:%S") + f".{frac:09d}Z"


def entry(rel, path, isdir):
    st = os.stat(path)
    e = {
        "Path": rel,
        "Name": rel.rsplit("/", 1)[-1],
        "Size": -1 if isdir else st.st_size,
        "IsDir": isdir,
        "ID": str(st.st_ino),
    }
    if not ctrl().get("omit_modtime"):
        e["ModTime"] = mtime_str(st)
    return e


def op_lsjson(rule):
    if rule and rule.get("synth"):
        n = rule["synth"]
        out = [
            {"Path": f"d{i % 100}/f{i}.txt", "Name": f"f{i}.txt", "Size": 10,
             "ModTime": "2026-01-01T00:00:00Z", "IsDir": False, "ID": str(i)}
            for i in range(n)
        ]
        sys.stdout.write(json.dumps(out))
        return 0
    pos = positional()
    target = resolve(pos[-1])
    rec, files_only, dirs_only = "-R" in args, "--files-only" in args, "--dirs-only" in args
    if os.path.isfile(target):
        sys.stdout.write(json.dumps([entry(os.path.basename(target), target, False)]))
        return 0
    if not os.path.isdir(target):
        die("Failed to lsjson: error in ListJSON: directory not found", 3)
    out = []

    def walk(d, rel):
        for name in sorted(os.listdir(d)):
            p = os.path.join(d, name)
            r = f"{rel}/{name}" if rel else name
            isdir = os.path.isdir(p)
            if isdir:
                if not files_only:
                    out.append(entry(r, p, True))
                if rec:
                    walk(p, r)
            elif not dirs_only:
                out.append(entry(r, p, False))

    walk(target, "")
    sys.stdout.write(json.dumps(out))
    return 0


def op_copyto():
    s, d = (resolve(x) for x in positional()[:2])
    if not os.path.isfile(s):
        die("Failed to copyto: error reading source root directory: directory not found", 3)
    os.makedirs(os.path.dirname(d) or ".", exist_ok=True)
    shutil.copy2(s, d)
    return 0


def op_moveto():
    s, d = (resolve(x) for x in positional()[:2])
    if not os.path.isfile(s):
        die("Failed to moveto: error reading source root directory: directory not found", 3)
    os.makedirs(os.path.dirname(d) or ".", exist_ok=True)
    shutil.move(s, d)
    return 0


def op_deletefile():
    p = resolve(positional()[0])
    if not os.path.isfile(p):
        die("Failed to deletefile: object not found", 4)
    os.remove(p)
    return 0


def op_cat():
    p = resolve(positional()[0])
    if not os.path.isfile(p):
        die("Failed to cat: object not found", 3)
    sys.stdout.flush()
    with open(p, "rb") as f:
        sys.stdout.buffer.write(f.read())
    return 0


def op_purge():
    p = resolve(positional()[0])
    if not os.path.isdir(p):
        die("Failed to purge: directory not found", 3)
    shutil.rmtree(p)
    return 0


def op_delete():
    # jak prawdziwy rclone: --min-age dotyczy CZASU MODYFIKACJI pliku (nie czasu trafienia do kosza)
    p = resolve(positional()[0])
    if not os.path.isdir(p):
        die("Failed to delete: directory not found", 3)
    age = flag_value("--min-age")
    limit = None
    if age:
        m = re.match(r"^(\d+)([smhdw])$", age)
        mult = {"s": 1, "m": 60, "h": 3600, "d": 86400, "w": 604800}[m.group(2)]
        limit = time.time() - int(m.group(1)) * mult
    for dp, dn, fn in os.walk(p):
        for f in fn:
            fp = os.path.join(dp, f)
            if limit is None or os.stat(fp).st_mtime < limit:
                os.remove(fp)
    if "--rmdirs" in args:
        for dp, dn, fn in os.walk(p, topdown=False):
            if dp != p and not os.listdir(dp):
                os.rmdir(dp)
    return 0


def main():
    if not args:
        die("usage", 1)
    op = args[0]
    rule = applicable_rule(op)
    delay = float(os.environ.get("FAKE_RCLONE_DELAY", "0") or 0)
    if delay:
        time.sleep(delay)
    if rule:
        if rule.get("sleep"):
            time.sleep(rule["sleep"])
        if "stdout" in rule:
            sys.stdout.write(rule["stdout"])
            sys.stdout.flush()
            if rule.get("code"):
                die(rule.get("stderr", ""), rule["code"])
            log_call()
            return 0
        if rule.get("code") or "stderr" in rule:
            die(rule.get("stderr", ""), rule.get("code", 1))
    if op == "lsjson":
        rc = op_lsjson(rule)
    elif op == "copyto":
        rc = op_copyto()
    elif op == "moveto":
        rc = op_moveto()
    elif op == "deletefile":
        rc = op_deletefile()
    elif op == "cat":
        rc = op_cat()
    elif op == "purge":
        rc = op_purge()
    elif op == "delete":
        rc = op_delete()
    elif op == "version":
        sys.stdout.write("rclone v1.99.0-fake\n")
        rc = 0
    elif op == "listremotes":
        sys.stdout.write("fake:  local\n")
        rc = 0
    elif op == "config":
        sys.stdout.write("{}")
        rc = 0
    else:
        die(f"unknown command {op}", 1)
    log_call()
    return rc


if __name__ == "__main__":
    sys.exit(main())
