import json
import logging
import socket
import threading
import time
import unittest
from rual import RualHandler


class AdapterTest(unittest.TestCase):
    def test_nonfinite_fields_are_discarded_before_sending(self):
        with self.assertRaises(ValueError):
            RualHandler._encode({'fields': {'value': float('nan')}})

    def test_authenticated_logging_and_bounded_offline_close(self):
        with socket.socket() as server:
            server.bind(('127.0.0.1', 0))
            server.listen()
            frames = []
            def receive():
                with server.accept()[0] as client, client.makefile() as reader:
                    frames.extend(json.loads(reader.readline()) for _ in range(2))
            worker = threading.Thread(target=receive)
            worker.start()
            handler = RualHandler('python-check', port=server.getsockname()[1], token='test-secret')
            logger = logging.getLogger('rual-test')
            logger.addHandler(handler)
            logger.error('one\ntwo', extra={'fields': {'request': 42}})
            handler.close()
            logger.removeHandler(handler)
            worker.join(3)
            self.assertFalse(worker.is_alive())
            self.assertEqual(frames[0]['token'], 'test-secret')
            self.assertEqual(frames[1]['message'], 'one\ntwo')
            self.assertEqual(frames[1]['fields']['request'], 42)
        handler = RualHandler('offline', port=server.getsockname()[1] if server.fileno() != -1 else 1)
        start = time.monotonic()
        for _ in range(500):
            handler.emit(logging.LogRecord('test', logging.INFO, '', 0, 'offline', (), None))
        handler.emit(logging.LogRecord('test', logging.INFO, '', 0, 'x' * 70000, (), None))
        self.assertGreater(handler.dropped, 0)
        handler.close()
        self.assertLess(time.monotonic() - start, 2)
        with self.assertRaises(ValueError):
            RualHandler('remote', host='192.168.1.2', token='')


if __name__ == '__main__':
    unittest.main()
