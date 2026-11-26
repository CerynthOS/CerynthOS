#include "monitor.h"
#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QCommandLineParser>
#include <QDir>
#include <QFile>
#include <QJsonDocument>
#include <QQuickWindow>
#include <QQuickStyle>
#include <QElapsedTimer>
#include <QJsonArray>
#include <QJsonObject>

int main(int argc,char **argv) {
    QQuickStyle::setStyle("Basic");
    QGuiApplication app(argc,argv);app.setApplicationName("Cerynth Observer");
    QCommandLineParser parser;parser.addHelpOption();
    parser.addOption({"agent-directory","Private owner agent state/socket directory","path",QDir::homePath()+"/.local/state/cerynth/agentd"});
    parser.addOption({"inference-directory","Private owner inference state/socket directory","path",QDir::homePath()+"/.local/state/cerynth/inferenced"});
    parser.addOption({"capture","Save a local fixture screenshot after refresh","path"});
    parser.addOption({"select-session","Initially selected session UUID","uuid"});
    parser.addOption({"snapshot-json","Export the observed public fixture snapshot","path"});
    parser.addOption({"frame-metrics","Measure rendered frame intervals during a bounded fixture","path"});
    parser.addOption({"exit-after-ms","Bounded demo/test duration","milliseconds"});parser.process(app);
    Monitor monitor(parser.value("agent-directory"),parser.value("inference-directory"));
    if (parser.isSet("select-session")) monitor.setSelectedSession(parser.value("select-session"));
    QQmlApplicationEngine engine;engine.rootContext()->setContextProperty("monitor",&monitor);
    engine.loadFromModule("Cerynth.Observer","Main");
    if (engine.rootObjects().isEmpty()) return 1;
    QTimer frameTimer;QElapsedTimer elapsed;QJsonArray intervals;int frameCount=0;qint64 lastFrame=0;
    if (parser.isSet("frame-metrics")) {
        if (!parser.isSet("exit-after-ms")) return 2;
        auto *window=qobject_cast<QQuickWindow *>(engine.rootObjects().first());
        if (!window) return 2;
        elapsed.start();frameTimer.setInterval(16);frameTimer.setTimerType(Qt::PreciseTimer);
        QObject::connect(&frameTimer,&QTimer::timeout,window,&QQuickWindow::update);
        QObject::connect(window,&QQuickWindow::frameSwapped,&app,[&] {
            const qint64 now=elapsed.nsecsElapsed();
            if (lastFrame) intervals.append(double(now-lastFrame)/1000000.0);
            lastFrame=now;++frameCount;
        });frameTimer.start();
    }
    if (parser.isSet("exit-after-ms")) {
        bool ok=false;const int duration=parser.value("exit-after-ms").toInt(&ok);
        if (!ok || duration<1000 || duration>60000) return 2;
        QTimer::singleShot(duration,&app,[&] {
            if (parser.isSet("frame-metrics")) {
                frameTimer.stop();QFile output(parser.value("frame-metrics"));
                if (!output.open(QIODevice::WriteOnly)) {app.exit(3);return;}
                output.setPermissions(QFile::ReadOwner|QFile::WriteOwner);
                output.write(QJsonDocument(QJsonObject{{"frames",frameCount},{"elapsed_ms",double(elapsed.nsecsElapsed())/1000000.0},
                    {"frame_intervals_ms",intervals},{"synthetic_repaint_timer",true},{"platform",QGuiApplication::platformName()}}).toJson());
            }
            if (parser.isSet("snapshot-json")) {
                QFile output(parser.value("snapshot-json"));
                if (output.open(QIODevice::WriteOnly)) {
                    output.setPermissions(QFile::ReadOwner|QFile::WriteOwner);
                    output.write(QJsonDocument::fromVariant(monitor.data()).toJson());
                } else { app.exit(3);return; }
            }
            if (parser.isSet("capture")) {
                auto *window=qobject_cast<QQuickWindow *>(engine.rootObjects().first());
                if (!window || !window->grabWindow().save(parser.value("capture"))) {app.exit(4);return;}
            }
            app.quit();
        });
    }
    return app.exec();
}
