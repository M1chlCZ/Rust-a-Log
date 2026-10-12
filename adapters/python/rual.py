"""A bounded, best-effort logging.Handler for rust-a-log. Standard library only."""
import datetime
import json
import logging
import os
import queue
import socket
import threading
import time


class RualHandler(logging.Handler):
    def __init__(self, source, host='127.0.0.1', port=7331, token=None):
        super().__init__()
        self.token = os.environ.get('RUAL_TOKEN', '') if token is None else token
        if host not in ('localhost', '127.0.0.1', '::1') and not self.token:
            raise ValueError('remote receiver requires a token')
        if not source or len(source.encode()) > 128 or not 1 <= port <= 65535 or len(self.token.encode()) > 256:
            raise ValueError('invalid source, port, or token')
        self.source, self.host, self.port = source, host, port
        self._queue = queue.Queue(64)
        self._closing = threading.Event()
        self._deadline = float('inf')
        self._dropped = 0
        self._counter = threading.Lock()
        self._worker = threading.Thread(target=self._send, name='rual-sender', daemon=True)
        self._worker.start()

    @property
    def dropped(self):
        with self._counter:
            return self._dropped

    def _discard(self, count=1):
        with self._counter:
            self._dropped += count

    def emit(self, record):
        try:
            fields = dict(getattr(record, 'fields', {}))
            fields['logger'] = record.name
            if record.exc_info:
                fields['exception'] = logging.Formatter().formatException(record.exc_info)
            frame = self._encode(dict(type='log', timestamp=datetime.datetime.fromtimestamp(
                record.created, datetime.timezone.utc).isoformat(), level=record.levelname,
                message=record.getMessage(), fields=fields))
            if len(frame) > 65536 or self._closing.is_set():
                self._discard()
            else:
                self._queue.put_nowait(frame)
        except Exception:
            self._discard()

    @staticmethod
    def _encode(value):
        return (json.dumps(value, ensure_ascii=False, allow_nan=False) + '\n').encode('utf-8')

    def _send(self):
        client, reported, retry = None, 0, 0
        try:
            while not self._closing.is_set() or (not self._queue.empty() and time.monotonic() < self._deadline):
                try:
                    frame = self._queue.get(timeout=0.02)
                except queue.Empty:
                    continue
                try:
                    if client is None:
                        if time.monotonic() < retry:
                            self._discard()
                            continue
                        client = socket.create_connection((self.host, self.port), timeout=0.1)
                        client.settimeout(0.2)
                        client.sendall(self._encode(dict(type='register', version=1,
                            source=self.source, language='python', token=self.token)))
                    lost = self.dropped
                    if lost > reported:
                        client.sendall(self._encode(dict(type='dropped', count=lost - reported)))
                    client.sendall(frame)
                    reported = lost
                except Exception:
                    self._discard()
                    if client is not None:
                        client.close()
                    client = None
                    retry = time.monotonic() + 0.2
        finally:
            if client is not None:
                client.close()
            self._discard(self._queue.qsize())

    def close(self):
        if hasattr(self, '_closing'):
            if not self._closing.is_set():
                self._deadline = time.monotonic() + 0.5
                self._closing.set()
            self._worker.join(timeout=1)
        super().close()
