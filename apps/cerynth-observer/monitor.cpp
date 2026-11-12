#include "monitor.h"
#include <QDateTime>
#include <QElapsedTimer>
#include <QDir>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLocalSocket>
#include <QtConcurrent>
#include <sys/socket.h>
#include <sys/stat.h>
#include <unistd.h>
#include <stdexcept>

namespace {
QJsonValue query(const QString &directory,const QString &socketName,const QJsonObject &operation) {
    const auto path=QDir(directory).filePath(socketName).toLocal8Bit();
    struct stat metadata{};
    if (::lstat(path.constData(),&metadata)!=0 || !S_ISSOCK(metadata.st_mode)
        || metadata.st_uid!=::getuid() || (metadata.st_mode & 0077)!=0)
        throw std::runtime_error("Private owner socket unavailable");
    QLocalSocket socket;
    socket.connectToServer(QString::fromLocal8Bit(path));
    if (!socket.waitForConnected(800)) throw std::runtime_error("Service unavailable");
    struct ucred peer{};socklen_t size=sizeof(peer);
    if (::getsockopt(socket.socketDescriptor(),SOL_SOCKET,SO_PEERCRED,&peer,&size)!=0
        || peer.uid!=::getuid()) throw std::runtime_error("Service identity mismatch");
    const auto frame=QJsonDocument(QJsonObject{{"version",1},{"operation",operation}}).toJson(QJsonDocument::Compact)+'\n';
    if (socket.write(frame)!=frame.size() || !socket.waitForBytesWritten(800))
        throw std::runtime_error("Service request failed");
    QByteArray bytes;
    QElapsedTimer deadline;deadline.start();
    while (!bytes.contains('\n')) {
        if (deadline.elapsed()>2500) throw std::runtime_error("Service response deadline exceeded");
        if (socket.bytesAvailable()==0 && !socket.waitForReadyRead(250)) continue;
        bytes+=socket.read(qMin<qint64>(socket.bytesAvailable(),4*1024*1024+1-bytes.size()));
        if (bytes.size()>4*1024*1024) throw std::runtime_error("Service response exceeds limit");
    }
    QJsonParseError error{};
    const auto envelope=QJsonDocument::fromJson(bytes.left(bytes.indexOf('\n')),&error).object();
    if (error.error!=QJsonParseError::NoError || envelope.value("version").toInt()!=1)
        throw std::runtime_error("Unsupported service response");
    const auto result=envelope.value("result").toObject();
    if (!result.contains("Ok")) throw std::runtime_error("Service rejected this read operation");
    return result.value("Ok");
}

QVariantMap collect(const QString &agent,const QString &inference,const QString &selected,
                    const QMap<QString,quint64> &oldCursors,const QVariantList &oldEvents,int offset) {
    QVariantMap output{{"updatedAt",QDateTime::currentDateTimeUtc().toString(Qt::ISODate)},
        {"stale",false},{"readOnly",true}};
    auto cursors=oldCursors;QVariantList events=oldEvents;
    try {
        const auto sessions=query(agent,"agentd.sock",{{"type","list"}}).toArray();
        output["sessions"]=sessions.toVariantList();
        int observed=0;
        QSet<QString> live;
        for (const auto &value:sessions) live.insert(value.toObject().value("id").toString());
        for (auto it=cursors.begin();it!=cursors.end();) {
            if (!live.contains(it.key())) it=cursors.erase(it);else ++it;
        }
        for (qsizetype index=0;index<sessions.size();++index) {
            const auto value=sessions.at((index+offset)%sessions.size());
            const auto session=value.toObject();const auto id=session.value("id").toString();
            // Bound the work per refresh. Selected session wins; remaining
            // cursors are retained and never represented as a complete audit.
            if (id!=selected && observed>=8) continue;
            const auto response=query(agent,"agentd.sock",{{"type","events"},{"id",id},
                {"after",static_cast<qint64>(cursors.value(id,0))}}).toObject();
            const auto oldest=response.value("oldest_sequence").toInteger();
            if (oldest>static_cast<qint64>(cursors.value(id,0))+1)
                events.append(QVariantMap{{"name","event_retention_gap"},{"category","diagnostic"},
                    {"session_id",id},{"attributes",QVariantMap{{"source","consumer"}}}});
            for (const auto &event:response.value("events").toArray()) events.append(event.toObject().toVariantMap());
            cursors[id]=static_cast<quint64>(response.value("last_sequence").toInteger());++observed;
        }
        output["eventsPartial"]=sessions.size()>observed;
        output["pollOffset"]=sessions.isEmpty() ? 0 : (offset+8)%sessions.size();
        while (events.size()>512) events.removeFirst();
        output["events"]=events;
        QVariantMap encoded;for (auto it=cursors.cbegin();it!=cursors.cend();++it) encoded[it.key()]=it.value();
        output["cursors"]=encoded;
        output["resources"]=query(agent,"agentd.sock",{{"type","resource_view"}}).toObject().toVariantMap();
        const auto gpu=query(agent,"agentd.sock",{{"type","gpu_jobs"}}).toObject();
        QVariantList devices,jobs;
        for (const auto &value:gpu.value("devices").toObject()) devices.append(value.toObject().toVariantMap());
        for (const auto &value:gpu.value("jobs").toObject()) jobs.append(value.toObject().toVariantMap());
        output["devices"]=devices;output["jobs"]=jobs;
        output["gpuPressureBlocked"]=gpu.value("pressure_blocked").toBool();
        if (!selected.isEmpty()) {
            try { output["accounting"]=query(agent,"agentd.sock",{{"type","resource_accounting"},{"id",selected}}).toObject().toVariantMap(); }
            catch (const std::exception &) { output["accountingUnavailable"]=true; }
        }
    } catch (const std::exception &error) {
        output["stale"]=true;output["agentError"]=QString::fromUtf8(error.what());
    }
    if (!inference.isEmpty()) {
        try {
            output["inference"]=query(inference,"control.sock",{{"type","health"}}).toObject().toVariantMap();
            output["metrics"]=query(inference,"control.sock",{{"type","metrics"}}).toObject().toVariantMap();
        } catch (const std::exception &) { output["inferenceUnavailable"]=true; }
    }
    return output;
}
}

Monitor::Monitor(QString agent,QString inference,QObject *parent)
    :QObject(parent),m_agentDirectory(std::move(agent)),m_inferenceDirectory(std::move(inference)) {
    connect(&m_watcher,&QFutureWatcher<QVariantMap>::finished,this,[this] {
        auto next=m_watcher.result();
        if (next.value("stale").toBool()) {
            // Retain the last known view with a visible stale flag.
            m_data["stale"]=true;m_data["agentError"]=next.value("agentError");
            m_data["inferenceUnavailable"]=next.value("inferenceUnavailable",true);
        } else {
            m_data=next;m_cursors.clear();
            const auto encoded=next.value("cursors").toMap();
            for (auto it=encoded.cbegin();it!=encoded.cend();++it) m_cursors[it.key()]=it.value().toULongLong();
        }
        emit changed();
    });
    m_data={{"stale",true},{"sessions",QVariantList{}},{"events",QVariantList{}},{"readOnly",true}};
    connect(&m_timer,&QTimer::timeout,this,&Monitor::refresh);m_timer.start(2000);
    QTimer::singleShot(0,this,&Monitor::refresh);
}
Monitor::~Monitor() { m_timer.stop();m_watcher.waitForFinished(); }
void Monitor::setSelectedSession(const QString &session) {
    if (m_selected==session) return;
    m_selected=session;emit changed();refresh();
}
void Monitor::refresh() {
    if (m_watcher.isRunning()) return;
    m_watcher.setFuture(QtConcurrent::run(collect,m_agentDirectory,m_inferenceDirectory,m_selected,
        m_cursors,m_data.value("events").toList(),m_data.value("pollOffset",0).toInt()));
    emit changed();
}
