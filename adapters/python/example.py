import logging
import os
from rual import RualHandler

handler = RualHandler('python-example', host=os.getenv('RUAL_HOST', '127.0.0.1'),
                      port=int(os.getenv('RUAL_PORT', '7331')))
logger = logging.getLogger('example')
logger.setLevel(logging.INFO)
logger.addHandler(handler)
logger.info('Application started', extra={'fields': {'port': 8080}})
logger.error('Request failed: demo-42', extra={'fields': {'request_id': 'demo-42'}})
handler.close()
logger.removeHandler(handler)
