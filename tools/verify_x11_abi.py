#!/usr/bin/env python3
"""Actual X11/XI2 header oracle. Does not connect to any display or input node."""
import os
import subprocess
import tempfile
from pathlib import Path

source = r'''
#include <stddef.h>
#include <X11/Xlib.h>
#include <X11/Xatom.h>
#include <X11/extensions/XInput2.h>
_Static_assert(sizeof(int) == 4 && sizeof(long) == 8 && sizeof(void *) == 8, "audited 64-bit X ABI");
_Static_assert(sizeof(XIDeviceInfo) == 40, "XI device size");
_Static_assert(offsetof(XIDeviceInfo, deviceid) == 0 && offsetof(XIDeviceInfo, name) == 8 && offsetof(XIDeviceInfo, use) == 16 && offsetof(XIDeviceInfo, attachment) == 20 && offsetof(XIDeviceInfo, enabled) == 24 && offsetof(XIDeviceInfo, num_classes) == 28 && offsetof(XIDeviceInfo, classes) == 32, "XI device offsets");
_Static_assert(sizeof(XIAnyClassInfo) == 8 && offsetof(XIAnyClassInfo, sourceid) == 4, "XI common class");
_Static_assert(sizeof(XIKeyClassInfo) == 24 && offsetof(XIKeyClassInfo, num_keycodes) == 8 && offsetof(XIKeyClassInfo, keycodes) == 16, "XI key class");
_Static_assert(sizeof(XIButtonClassInfo) == 40 && offsetof(XIButtonClassInfo, num_buttons) == 8 && offsetof(XIButtonClassInfo, labels) == 16 && offsetof(XIButtonClassInfo, state) == 24, "XI button class");
_Static_assert(XA_STRING == 31, "node string type");
_Static_assert(XISlavePointer == 3 && XISlaveKeyboard == 4 && XIKeyClass == 0 && XIButtonClass == 1 && XIAllDevices == 0, "XI constants");
void signatures(void) {
    XIDeviceInfo *(*query)(Display *,int,int *) = XIQueryDevice;
    Status (*property)(Display *,int,Atom,long,long,Bool,Atom,Atom *,int *,unsigned long *,unsigned long *,unsigned char **) = XIGetProperty;
    (void)query; (void)property;
}
'''
with tempfile.TemporaryDirectory(prefix="keyweave-x11-abi-") as directory:
    root = Path(directory)
    (root / "probe.c").write_text(source)
    result = subprocess.run([os.environ.get("CC", "cc"), "-std=c11", "-Wall", "-Wextra", "-Werror", "-c", str(root / "probe.c"), "-o", str(root / "probe.o")], capture_output=True, text=True)
    if result.returncode:
        print("X11 SDK oracle unavailable or failed; this is not an ABI pass")
        print(result.stderr.strip())
        raise SystemExit(1)
print("Verified actual X11/XI2 SDK layouts, constants and signatures; compile-only, no display/device access")
