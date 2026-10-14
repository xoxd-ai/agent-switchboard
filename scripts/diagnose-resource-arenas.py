#!/usr/bin/env python3
"""Small Linux-only allocator diagnostic; no build, broker, or process signals.

Authority: LAB-TAKEOVER-20260930, SWB-R49, R-N13.
"""
import argparse
import ctypes
import datetime
import json
import os
from pathlib import Path
import platform
import resource
import subprocess
import sys
import threading


def child():
    limit = 6 * 1024**3
    _, hard = resource.getrlimit(resource.RLIMIT_AS)
    if hard != resource.RLIM_INFINITY:
        limit = min(limit, hard)
    resource.setrlimit(resource.RLIMIT_AS, (limit, limit))
    os.sched_setaffinity(0, {min(os.sched_getaffinity(0))})
    threading.stack_size(512 * 1024)
    libc = ctypes.CDLL(None)
    libc.gnu_get_libc_version.restype = ctypes.c_char_p
    linked_glibc = libc.gnu_get_libc_version().decode()
    libc.malloc.argtypes = [ctypes.c_size_t]
    libc.malloc.restype = ctypes.c_void_p
    libc.free.argtypes = [ctypes.c_void_p]
    release = threading.Event()
    started = []
    allocation_failures = []
    failure = None
    def worker(ready):
        pointer = libc.malloc(4096)
        if not pointer:
            allocation_failures.append(True)
        ready.set()
        try:
            release.wait()
        finally:
            if pointer:
                libc.free(pointer)
    try:
        for _ in range(128):
            ready = threading.Event()
            try:
                thread = threading.Thread(target=worker, args=(ready,))
                thread.start()
            except (RuntimeError, MemoryError) as error:
                failure = str(error)
                break
            started.append(thread)
            ready.wait()
        values = {}
        for line in Path('/proc/self/status').read_text().splitlines():
            key, _, value = line.partition(':')
            if key in {'VmPeak', 'VmSize', 'VmRSS', 'Threads'}:
                values[key] = value.strip()
        result = dict(memory_limit_bytes=limit, linked_glibc=linked_glibc, threads_started=len(started),
                      requested_threads=128, thread_stack_bytes=512*1024,
                      allocation_bytes_per_thread=4096,
                      allocation_failures=len(allocation_failures),
                      thread_creation_error=failure, status=values)
    finally:
        release.set()
        for thread in started:
            thread.join()
    print(json.dumps(result, sort_keys=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--child', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    if platform.system() != 'Linux':
        parser.error('resource diagnostic runs on Honey or Sting, never Neo')
    if args.child:
        child()
        return
    if not args.output:
        parser.error('a new output file is required')
    # Open before observing to refuse accidental overwrite.
    with args.output.open('x') as output:
        runs = {}
        for mode in ('default', 'two-arenas'):
            env = dict(os.environ)
            env.pop('MALLOC_ARENA_MAX', None)
            env.pop('GLIBC_TUNABLES', None)
            if mode == 'two-arenas':
                env['MALLOC_ARENA_MAX'] = '2'
            result = subprocess.run([sys.executable, str(Path(__file__).resolve()), '--child'],
                                    env=env, text=True, capture_output=True, check=False)
            runs[mode] = {'exit_code': result.returncode}
            if result.returncode == 0:
                runs[mode]['observation'] = json.loads(result.stdout)
            else:
                runs[mode]['error'] = 'owned diagnostic child failed; no environment or stderr exported'
        receipt = dict(schema='swb.allocator-diagnostic.v1',
                       finished_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                       source_sha=subprocess.check_output(['git', '-C', str(Path(__file__).resolve().parents[1]), 'rev-parse', 'HEAD'], text=True).strip(),
                       runs=runs, live_acceptance=False, rust_fixture_execution=False,
                       process_signals_sent=False,
                       rulings=['LAB-TAKEOVER-20260930', 'SWB-R49', 'R-N13'])
        output.write(json.dumps(receipt, indent=2, sort_keys=True)+'\n')
        print(json.dumps(receipt, sort_keys=True))


if __name__ == '__main__':
    main()
