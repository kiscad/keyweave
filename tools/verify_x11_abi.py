#!/usr/bin/env python3
"""Actual X11/XI2 header oracle. Does not connect to any display or input node."""
import argparse
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
_Static_assert(sizeof(XIValuatorClassInfo) == 56 && offsetof(XIValuatorClassInfo, number) == 8 && offsetof(XIValuatorClassInfo, label) == 16 && offsetof(XIValuatorClassInfo, min) == 24 && offsetof(XIValuatorClassInfo, max) == 32 && offsetof(XIValuatorClassInfo, value) == 40 && offsetof(XIValuatorClassInfo, resolution) == 48 && offsetof(XIValuatorClassInfo, mode) == 52, "XI valuator class");
_Static_assert(XIValuatorClass == 2 && XIModeRelative == 0 && XIModeAbsolute == 1, "XI valuator constants");
_Static_assert(sizeof(XIScrollClassInfo) == 32 && offsetof(XIScrollClassInfo, number) == 8 && offsetof(XIScrollClassInfo, scroll_type) == 12 && offsetof(XIScrollClassInfo, increment) == 16 && offsetof(XIScrollClassInfo, flags) == 24, "XI scroll layout");
_Static_assert(XIScrollClass == 3 && XIScrollTypeVertical == 1 && XIScrollTypeHorizontal == 2 && XIScrollFlagNoEmulation == 1 && XIScrollFlagPreferred == 2, "XI scroll constants");
_Static_assert(XA_STRING == 31, "node string type");
_Static_assert(XISlavePointer == 3 && XISlaveKeyboard == 4 && XIKeyClass == 0 && XIButtonClass == 1 && XIAllDevices == 0, "XI constants");
void signatures(void) {
    XIDeviceInfo *(*query)(Display *,int,int *) = XIQueryDevice;
    Status (*property)(Display *,int,Atom,long,long,Bool,Atom,Atom *,int *,unsigned long *,unsigned long *,unsigned char **) = XIGetProperty;
    Display *(*open_display)(const char *) = XOpenDisplay;
    int (*close_display)(Display *) = XCloseDisplay;
    int (*free_data)(void *) = XFree;
    Atom (*atom)(Display *, const char *, Bool) = XInternAtom;
    Status (*version)(Display *, int *, int *) = XIQueryVersion;
    void (*free_devices)(XIDeviceInfo *) = XIFreeDeviceInfo;
    (void)query; (void)property; (void)open_display; (void)close_display;
    (void)free_data; (void)atom; (void)version; (void)free_devices;
}
'''
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--include", action="append", type=Path, default=[])
args = parser.parse_args()
include_args = []
for directory in args.include:
    include_args.extend(["-I", str(directory.resolve(strict=True))])
with tempfile.TemporaryDirectory(prefix="keyweave-x11-abi-") as directory:
    root = Path(directory)
    (root / "probe.c").write_text(source)
    result = subprocess.run([os.environ.get("CC", "cc"), "-std=c11", "-Wall", "-Wextra", "-Werror", *include_args, "-c", str(root / "probe.c"), "-o", str(root / "probe.o")], capture_output=True, text=True)
    if result.returncode:
        print("X11 SDK oracle unavailable or failed; this is not an ABI pass")
        print(result.stderr.strip())
        raise SystemExit(1)
print("Verified actual X11/XI2 SDK layouts, constants and signatures; compile-only, no display/device access")
