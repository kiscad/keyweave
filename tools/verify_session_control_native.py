#!/usr/bin/env python3
"""Explicit native AF_UNIX gate. No input devices, global capture or injection.

These tests use private temporary sockets and owned status/stop CLI processes.
One explicitly built owned no-input worker fixture checks the host lifecycle,
without generating any progress receipt. Default workspace tests intentionally ignore them in socket-restricted sandboxes.
"""
import hashlib
import json
import os
import subprocess
from pathlib import Path
root=Path(__file__).resolve().parents[1]
logs=root/'target/validation/local-control-native'
logs.mkdir(parents=True,exist_ok=True)
commands=[['cargo','build','-p','keyweave-host','--example','session-control-fixture','--locked','--offline'],['cargo','test','-p','keyweave-linux','--lib','session_control','--locked','--offline','--','--ignored'],['cargo','test','-p','keyweave-host','--test','local_control','--locked','--offline','--','--ignored'],['cargo','test','-p','keyweave-ui','--lib','actual_gui_client','--locked','--offline','--','--ignored']]
results=[]
for i,command in enumerate(commands):
    log=logs/f'{i}.log'
    with log.open('w') as output:
        result=subprocess.run(command,cwd=root,env={**os.environ,'RUSTUP_TOOLCHAIN':'stable','KEYWEAVE_CONTROL_FIXTURE':str(root/'target/debug/examples/session-control-fixture')},stdout=output,stderr=subprocess.STDOUT)
    results.append({'command':command,'exit_code':result.returncode,'log':str(log.relative_to(root)),'sha256':hashlib.sha256(log.read_bytes()).hexdigest()})
    (logs/'results.json').write_text(json.dumps(results,indent=2)+'\n')
    print(json.dumps(results[-1]),flush=True)
    if result.returncode:raise SystemExit(result.returncode)
