#pragma once

#include <QHash>
#include <QImage>
#include <QJsonObject>
#include <QLocalSocket>
#include <QObject>
#include <QProcess>
#include <QQueue>
#include <QTimer>
#include <QVariant>

class StoreClient final : public QObject
{
    Q_OBJECT
    Q_PROPERTY(bool ready READ ready NOTIFY readyChanged)
    Q_PROPERTY(QString backendBinary READ backendBinary CONSTANT)
    Q_PROPERTY(QString socketPath READ socketPath CONSTANT)

public:
    explicit StoreClient(QObject *parent = nullptr);
    ~StoreClient() override;

    bool ready() const;
    QString backendBinary() const;
    QString socketPath() const;

    Q_INVOKABLE void start();
    Q_INVOKABLE void request(const QString &method, const QVariantMap &params = {});
    Q_INVOKABLE QString iconSource(const QVariantList &candidates) const;

    // Theme icon rendered by iconSource(). Safe from any thread, unlike
    // QIcon, which image providers are called on for asynchronous images.
    static QImage themeIcon(const QString &name);

signals:
    void readyChanged();
    void response(const QString &method, const QVariant &result);
    void requestFailed(const QString &method, const QString &message);
    void notification(const QString &method, const QVariant &params);
    void backendLog(const QString &message);

private slots:
    void attemptConnection();
    void handleConnected();
    void handleDisconnected();
    void handleSocketData();
    void handleBackendOutput();

private:
    void setReady(bool ready);
    void flushRequests();
    void processMessage(const QJsonObject &message);

    QProcess m_backend;
    QLocalSocket m_socket;
    QTimer m_retryTimer;
    QByteArray m_inputBuffer;
    QQueue<QByteArray> m_outgoing;
    QHash<qint64, QString> m_methods;
    qint64 m_nextRequestId = 1;
    int m_connectionAttempts = 0;
    bool m_ready = false;
    bool m_startedBackend = false;
    bool m_shuttingDown = false;
};
