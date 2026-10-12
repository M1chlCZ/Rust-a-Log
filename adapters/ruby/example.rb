require 'logger'
require_relative 'rual'

output = Rual::LogDevice.new('ruby-example', host: ENV.fetch('RUAL_HOST', '127.0.0.1'),
                           port: ENV.fetch('RUAL_PORT', '7331').to_i)
logger = Logger.new(output)
logger.info('Application started')
logger.error('Request failed: demo-42')
logger.close
