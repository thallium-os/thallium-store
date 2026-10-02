#include "storeclient.h"

#include <QDir>
#include <QIcon>
#include <QIODevice>
#include <QJsonDocument>
#include <QJsonParseError>
#include <QList>
#include <QUrl>

namespace {
constexpr int kConnectionIntervalMs = 100;
constexpr int kMaximumConnectionAttempts = 80;
}

StoreClient::StoreClient(QObject *parent)
    : QObject(parent)
{
    m_retryTimer.setInterval(kConnectionIntervalMs);
    connect(&m_retryTimer, &QTimer::timeout, this, &StoreClient::attemptConnection);
    connect(&m_socket, &QLocalSocket::connected, this, &StoreClient::handleConnected);
    connect(&m_socket, &QLocalSocket::disconnected, this, &StoreClient::handleDisconnected);
    connect(&m_socket, &QLocalSocket::readyRead, this, &StoreClient::handleSocketData);
    connect(&m_backend, &QProcess::readyReadStandardError, this, &StoreClient::handleBackendOutput);
    connect(&m_backend, &QProcess::readyReadStandardOutput, this, &StoreClient::handleBackendOutput);
    connect(&m_backend, &QProcess::errorOccurred, this, [this](QProcess::ProcessError error) {
        if (error == QProcess::FailedToStart) {
            emit backendLog(QStringLiteral("Could not start %1: %2")
                                .arg(backendBinary(), m_backend.errorString()));
        }
    });
}

StoreClient::~StoreClient()
{
    m_shuttingDown = true;
    m_retryTimer.stop();
    m_socket.abort();
    if (m_startedBackend && m_backend.state() != QProcess::NotRunning) {
        m_backend.terminate();
        if (!m_backend.waitForFinished(1500)) {
            m_backend.kill();
            m_backend.waitForFinished(500);
        }
    }
}

bool StoreClient::ready() const
{
    return m_ready;
}

QString StoreClient::backendBinary() const
{
    return qEnvironmentVariable("THALLIUM_STORE_BACKEND", QStringLiteral("thallium-store-backend"));
}

QString StoreClient::socketPath() const
{
    const QString explicitPath = qEnvironmentVariable("THALLIUM_STORE_SOCKET");
    if (!explicitPath.isEmpty()) {
        return explicitPath;
    }

    QString runtimeDirectory = qEnvironmentVariable("XDG_RUNTIME_DIR");
    if (runtimeDirectory.isEmpty()) {
        runtimeDirectory = QStringLiteral("/tmp");
    }
    return QDir(runtimeDirectory).filePath(QStringLiteral("thallium-store/backend.sock"));
}

void StoreClient::start()
{
    if (m_retryTimer.isActive() || m_socket.state() == QLocalSocket::ConnectedState) {
        return;
    }

    // Starting a helper is safe when another daemon already owns the socket:
    // the Rust backend detects that instance and exits immediately.
    if (m_backend.state() == QProcess::NotRunning) {
        m_backend.setProgram(backendBinary());
        m_backend.start();
        m_startedBackend = true;
    }
    m_connectionAttempts = 0;
    m_retryTimer.start();
    attemptConnection();
}

void StoreClient::request(const QString &method, const QVariantMap &params)
{
    const qint64 id = m_nextRequestId++;
    m_methods.insert(id, method);

    QJsonObject requestObject{
        {QStringLiteral("jsonrpc"), QStringLiteral("2.0")},
        {QStringLiteral("id"), id},
        {QStringLiteral("method"), method},
        {QStringLiteral("params"), QJsonObject::fromVariantMap(params)},
    };
    QByteArray payload = QJsonDocument(requestObject).toJson(QJsonDocument::Compact);
    payload.append('\n');
    m_outgoing.enqueue(payload);

    if (m_socket.state() == QLocalSocket::ConnectedState) {
        flushRequests();
    } else {
        start();
    }
}

QString StoreClient::iconSource(const QVariantList &candidates) const
{
    for (const QVariant &candidate : candidates) {
        const QString name = candidate.toString().trimmed();
        if (!name.isEmpty() && QIcon::hasThemeIcon(name)) {
            return QStringLiteral("image://theme/%1")
                .arg(QString::fromUtf8(QUrl::toPercentEncoding(name)));
        }
    }
    return {};
}

void StoreClient::attemptConnection()
{
    if (m_socket.state() == QLocalSocket::ConnectedState
        || m_socket.state() == QLocalSocket::ConnectingState) {
        return;
    }

    if (++m_connectionAttempts > kMaximumConnectionAttempts) {
        m_retryTimer.stop();
        const QString message = QStringLiteral("Backend socket did not become ready: %1")
                                    .arg(socketPath());
        emit backendLog(message);
        const QList<QString> pendingMethods = m_methods.values();
        m_methods.clear();
        m_outgoing.clear();
        for (const QString &method : pendingMethods) {
            emit requestFailed(method, message);
        }
        return;
    }

    m_socket.abort();
    m_socket.connectToServer(socketPath(), QIODevice::ReadWrite);
}

void StoreClient::handleConnected()
{
    m_retryTimer.stop();
    setReady(true);
    emit backendLog(QStringLiteral("Connected to native backend"));
    flushRequests();
}

void StoreClient::handleDisconnected()
{
    setReady(false);
    if (!m_shuttingDown) {
        m_connectionAttempts = 0;
        m_retryTimer.start();

        const QString message = QStringLiteral("Backend connection closed; reconnecting");
        const QList<QString> pendingMethods = m_methods.values();
        m_methods.clear();
        m_outgoing.clear();
        for (const QString &method : pendingMethods) {
            emit requestFailed(method, message);
        }
    }
}

void StoreClient::handleSocketData()
{
    m_inputBuffer.append(m_socket.readAll());
    qsizetype newline = -1;
    while ((newline = m_inputBuffer.indexOf('\n')) >= 0) {
        const QByteArray line = m_inputBuffer.left(newline).trimmed();
        m_inputBuffer.remove(0, newline + 1);
        if (line.isEmpty()) {
            continue;
        }

        QJsonParseError parseError;
        const QJsonDocument document = QJsonDocument::fromJson(line, &parseError);
        if (parseError.error != QJsonParseError::NoError || !document.isObject()) {
            emit backendLog(QStringLiteral("Invalid backend response: %1")
                                .arg(parseError.errorString()));
            continue;
        }
        processMessage(document.object());
    }
}

void StoreClient::handleBackendOutput()
{
    const QByteArray output = m_backend.readAllStandardError() + m_backend.readAllStandardOutput();
    for (const QByteArray &line : output.split('\n')) {
        const QString message = QString::fromUtf8(line).trimmed();
        if (!message.isEmpty()) {
            emit backendLog(message);
        }
    }
}

void StoreClient::setReady(bool ready)
{
    if (m_ready == ready) {
        return;
    }
    m_ready = ready;
    emit readyChanged();
}

void StoreClient::flushRequests()
{
    while (!m_outgoing.isEmpty() && m_socket.state() == QLocalSocket::ConnectedState) {
        m_socket.write(m_outgoing.dequeue());
    }
    m_socket.flush();
}

void StoreClient::processMessage(const QJsonObject &message)
{
    if (message.contains(QStringLiteral("method")) && !message.contains(QStringLiteral("id"))) {
        emit notification(message.value(QStringLiteral("method")).toString(),
                          message.value(QStringLiteral("params")).toVariant());
        return;
    }

    const qint64 id = message.value(QStringLiteral("id")).toVariant().toLongLong();
    const QString method = m_methods.take(id);
    if (method.isEmpty()) {
        return;
    }

    const QJsonValue error = message.value(QStringLiteral("error"));
    if (error.isObject()) {
        emit requestFailed(method,
                           error.toObject().value(QStringLiteral("message")).toString(
                               QStringLiteral("Backend request failed")));
        return;
    }
    emit response(method, message.value(QStringLiteral("result")).toVariant());
}
