require 'minitest/autorun'
require 'socket'
require 'json'
require 'logger'
require_relative 'rual'

class AdapterTest < Minitest::Test
  def test_wire_format_and_offline_limits
    server = TCPServer.new('127.0.0.1', 0)
    received = Thread.new do
      client = server.accept
      frames = 2.times.map { JSON.parse(client.gets) }
      client.close
      frames
    end
    output = Rual::LogDevice.new('ruby-check', port: server.addr[1], token: 'test-secret')
    Logger.new(output).error('one two')
    output.close
    assert received.join(3), 'sender did not finish'
    frames = received.value
    assert_equal 'test-secret', frames[0]['token']
    assert_equal 'ERROR', frames[1]['level']
    assert_includes frames[1]['message'], 'one two'
    server.close
    output = Rual::LogDevice.new('offline', port: 1)
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    500.times { output.log('INFO', 'offline') }
    output.log('INFO', 'x' * 70000)
    assert_operator output.dropped, :>, 0
    output.close
    assert_operator Process.clock_gettime(Process::CLOCK_MONOTONIC) - started, :<, 2
    assert_raises(ArgumentError) { Rual::LogDevice.new('remote', host: '192.168.1.2', token: '') }
  end
end
