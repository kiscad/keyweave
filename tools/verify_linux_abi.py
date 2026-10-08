#!/usr/bin/env python3
"""Read-only native header ABI probe; never opens input/uinput or calls ioctl."""
import json
import os
import subprocess
import tempfile
from pathlib import Path

source = r'''
#include <stdio.h>
#include <stddef.h>
#include <linux/input.h>
int main(void) {
    printf("{\"event_size\":%zu,\"type_offset\":%zu,\"code_offset\":%zu,"
           "\"value_offset\":%zu,\"id_size\":%zu,\"word_size\":%zu,"
           "\"id_request\":%lu,\"event_bits_request\":%lu,"
           "\"key_state_request\":%lu,\"clock_request\":%lu}\n",
           sizeof(struct input_event), offsetof(struct input_event, type),
           offsetof(struct input_event, code), offsetof(struct input_event, value),
           sizeof(struct input_id), sizeof(unsigned long), (unsigned long)EVIOCGID,
           (unsigned long)EVIOCGBIT(0,8), (unsigned long)EVIOCGKEY(96),
           (unsigned long)EVIOCSCLOCKID);
    return 0;
}
'''
expected = {'event_size': 24, 'type_offset': 16, 'code_offset': 18,
            'value_offset': 20, 'id_size': 8, 'word_size': 8,
            'id_request': 0x80084502, 'event_bits_request': 0x80084520,
            'key_state_request': 0x80604518, 'clock_request': 0x400445a0}
with tempfile.TemporaryDirectory(prefix='keyweave-abi-') as directory:
    root = Path(directory)
    (root / 'probe.c').write_text(source)
    subprocess.run([os.environ.get('CC', 'cc'), '-Wall', '-Wextra', '-Werror',
                    str(root / 'probe.c'), '-o', str(root / 'probe')], check=True)
    result = subprocess.run([str(root / 'probe')], check=True, capture_output=True, text=True)
    actual = json.loads(result.stdout)
    if actual != expected:
        raise SystemExit(f'Unaudited native ABI: {actual}')
print('Verified local C UAPI sizes, offsets and evdev request encodings; no device access')
