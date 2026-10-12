package io.github.m1chlcz.rual;

import ch.qos.logback.classic.spi.ILoggingEvent;
import ch.qos.logback.classic.spi.ThrowableProxyUtil;
import ch.qos.logback.core.AppenderBase;
import java.util.HashMap;

/** Logback adapter, configurable through the existing logback.xml. */
public final class RualAppender extends AppenderBase<ILoggingEvent> {
    private String source = "java-app", host = "127.0.0.1";
    private int port = 7331;
    private RualClient client;
    public void setSource(String source) { this.source = source; }
    public void setHost(String host) { this.host = host; }
    public void setPort(int port) { this.port = port; }
    @Override public void start() {
        try { client = new RualClient(source, host, port); super.start(); }
        catch (Exception error) { addError("Cannot start rual adapter", error); }
    }
    @Override protected void append(ILoggingEvent event) {
        var fields = new HashMap<String, Object>(event.getMDCPropertyMap());
        fields.put("logger", event.getLoggerName());
        fields.put("thread", event.getThreadName());
        if (event.getThrowableProxy() != null) fields.put("exception", ThrowableProxyUtil.asString(event.getThrowableProxy()));
        client.log(event.getLevel().toString(), event.getFormattedMessage(), fields);
    }
    public long dropped() { return client == null ? 0 : client.dropped(); }
    @Override public void stop() { super.stop(); if (client != null) client.close(); }
}
