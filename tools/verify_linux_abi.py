#!/usr/bin/env python3
"""Read-only native header ABI probe; never opens input/uinput or calls ioctl."""
import json
import os
import subprocess
import tempfile
from pathlib import Path

source = r'''
#define _GNU_SOURCE
#include <stdio.h>
#include <stddef.h>
#include <linux/input.h>
#include <linux/uinput.h>
#include <sys/mman.h>
#include <fcntl.h>
#include <unistd.h>
#include <termios.h>
#include <sys/random.h>
#include <time.h>
_Static_assert(CLOCK_MONOTONIC == 1 && CLOCK_BOOTTIME == 7, "continuity clock IDs");
#include <sys/signalfd.h>
#include <signal.h>
#include <pthread.h>
_Static_assert(sizeof(struct signalfd_siginfo) == 128 && offsetof(struct signalfd_siginfo, ssi_signo) == 0, "signal record");
_Static_assert(SFD_CLOEXEC == O_CLOEXEC && SFD_NONBLOCK == O_NONBLOCK, "signal descriptor flags");
_Static_assert(GRND_NONBLOCK == 1, "nonblocking entropy flag");
#include <sys/ioctl.h>
#include <linux/kd.h>
#include <linux/vt.h>
_Static_assert(VT_GETSTATE == 0x5603 && KDGETMODE == 0x4b3b && KDGKBMODE == 0x4b44, "VT queries");
_Static_assert(KD_TEXT == 0 && K_XLATE == 1 && K_UNICODE == 3, "VT modes");
_Static_assert(sizeof(struct vt_stat) == 6 && _Alignof(struct vt_stat) == 2, "VT struct");
_Static_assert(offsetof(struct vt_stat, v_active) == 0 && offsetof(struct vt_stat, v_signal) == 2 && offsetof(struct vt_stat, v_state) == 4, "VT offsets");
_Static_assert(sizeof(pid_t) == 4 && sizeof(int) == 4 && O_NOCTTY == 0x100, "VT scalar types");
_Static_assert(MFD_CLOEXEC == 1 && MFD_ALLOW_SEALING == 2, "memfd flags");
_Static_assert(F_ADD_SEALS == 1033 && F_GET_SEALS == 1034, "sealing commands");
_Static_assert(F_SEAL_SEAL == 1 && F_SEAL_SHRINK == 2 && F_SEAL_GROW == 4 && F_SEAL_WRITE == 8, "seals");
_Static_assert(O_RDONLY == 0 && O_RDWR == 2 && O_ACCMODE == 3 && FD_CLOEXEC == 1, "descriptor flags");
_Static_assert(F_DUPFD_CLOEXEC == 1030 && F_GETFD == 1 && F_SETFD == 2, "descriptor handoff");
int main(void) {
    int (*read_clock)(clockid_t,struct timespec *) = clock_gettime;
    (void)read_clock;
    int (*signals_fd)(int,const sigset_t *,int) = signalfd;
    int (*thread_mask)(int,const sigset_t *,sigset_t *) = pthread_sigmask;
    int (*process_mask)(int,const sigset_t *,sigset_t *) = sigprocmask;
    int (*action_query)(int,const struct sigaction *,struct sigaction *) = sigaction;
    pid_t (*thread_id)(void) = gettid;
    (void)signals_fd; (void)thread_mask; (void)process_mask; (void)action_query; (void)thread_id;
    int (*create_memory)(const char *, unsigned int) = memfd_create;
    int (*control_fd)(int, int, ...) = fcntl;
    (void)create_memory; (void)control_fd;
    pid_t (*tty_session)(int) = tcgetsid;
    pid_t (*tty_foreground)(int) = tcgetpgrp;
    pid_t (*process_session)(pid_t) = getsid;
    pid_t (*process_group)(pid_t) = getpgid;
    pid_t (*own_group)(void) = getpgrp;
    pid_t (*own_pid)(void) = getpid;
    (void)own_group; (void)own_pid;
    int (*query)(int, unsigned long, ...) = ioctl;
    ssize_t (*entropy)(void *, size_t, unsigned int) = getrandom;
    (void)entropy;
    (void)tty_session; (void)tty_foreground; (void)process_session;
    (void)process_group; (void)query;
    printf("{\"event_size\":%zu,\"type_offset\":%zu,\"code_offset\":%zu,"
           "\"value_offset\":%zu,\"id_size\":%zu,\"word_size\":%zu,"
           "\"id_request\":%lu,\"event_bits_request\":%lu,"
           "\"key_state_request\":%lu,\"clock_request\":%lu,\"setup_size\":%zu,\"name_offset\":%zu,"
           "\"ff_offset\":%zu,\"setup_request\":%lu,\"version_request\":%lu,"
           "\"key_bit_request\":%lu,\"sysname_request\":%lu,\"create_request\":%lu,\"destroy_request\":%lu,\"grab_request\":%lu}\n",
           sizeof(struct input_event), offsetof(struct input_event, type),
           offsetof(struct input_event, code), offsetof(struct input_event, value),
           sizeof(struct input_id), sizeof(unsigned long), (unsigned long)EVIOCGID,
           (unsigned long)EVIOCGBIT(0,8), (unsigned long)EVIOCGKEY(96),
           (unsigned long)EVIOCSCLOCKID, sizeof(struct uinput_setup),
           offsetof(struct uinput_setup, name), offsetof(struct uinput_setup, ff_effects_max),
           (unsigned long)UI_DEV_SETUP, (unsigned long)UI_GET_VERSION,
           (unsigned long)UI_SET_KEYBIT, (unsigned long)UI_GET_SYSNAME(128),
           (unsigned long)UI_DEV_CREATE, (unsigned long)UI_DEV_DESTROY, (unsigned long)EVIOCGRAB);
    return 0;
}
'''
expected = {'event_size': 24, 'type_offset': 16, 'code_offset': 18,
            'value_offset': 20, 'id_size': 8, 'word_size': 8,
            'id_request': 0x80084502, 'event_bits_request': 0x80084520,
            'key_state_request': 0x80604518, 'clock_request': 0x400445a0,
            'setup_size': 92, 'name_offset': 8, 'ff_offset': 88,
            'setup_request': 0x405c5503, 'version_request': 0x8004552d,
            'key_bit_request': 0x40045565, 'sysname_request': 0x8080552c,
            'create_request': 0x5501, 'destroy_request': 0x5502, 'grab_request': 0x40044590}
with tempfile.TemporaryDirectory(prefix='keyweave-abi-') as directory:
    root = Path(directory)
    (root / 'probe.c').write_text(source)
    subprocess.run([os.environ.get('CC', 'cc'), '-Wall', '-Wextra', '-Werror',
                    str(root / 'probe.c'), '-o', str(root / 'probe')], check=True)
    result = subprocess.run([str(root / 'probe')], check=True, capture_output=True, text=True)
    actual = json.loads(result.stdout)
    if actual != expected:
        raise SystemExit(f'Unaudited native ABI: {actual}')
print('Verified local C UAPI, evdev/uinput, memfd/seals, VT/job-control getrandom, signalfd and paired-clock ABI; no device access')
