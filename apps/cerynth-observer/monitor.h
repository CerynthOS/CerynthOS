#pragma once
#include <QObject>
#include <QVariantMap>
#include <QFutureWatcher>
#include <QTimer>

class Monitor final : public QObject {
    Q_OBJECT
    Q_PROPERTY(QVariantMap data READ data NOTIFY changed)
    Q_PROPERTY(bool refreshing READ refreshing NOTIFY changed)
    Q_PROPERTY(QString selectedSession READ selectedSession WRITE setSelectedSession NOTIFY changed)
public:
    explicit Monitor(QString agentDirectory, QString inferenceDirectory, QObject *parent=nullptr);
    ~Monitor() override;
    QVariantMap data() const { return m_data; }
    bool refreshing() const { return m_watcher.isRunning(); }
    QString selectedSession() const { return m_selected; }
    void setSelectedSession(const QString &session);
    Q_INVOKABLE void refresh();
signals:
    void changed();
private:
    QString m_agentDirectory, m_inferenceDirectory, m_selected;
    QVariantMap m_data;
    QMap<QString,quint64> m_cursors;
    QFutureWatcher<QVariantMap> m_watcher;
    QTimer m_timer;
};
