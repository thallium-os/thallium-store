#include "storeclient.h"

#include <QGuiApplication>
#include <QIcon>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QQuickImageProvider>
#include <QUrl>

class ThemeIconProvider final : public QQuickImageProvider
{
public:
    ThemeIconProvider()
        : QQuickImageProvider(QQuickImageProvider::Image)
    {
    }

    QImage requestImage(const QString &id, QSize *size, const QSize &requestedSize) override
    {
        const QString iconName = QUrl::fromPercentEncoding(id.toUtf8());
        const QSize target = requestedSize.isValid() ? requestedSize : QSize(128, 128);
        const QImage image = QIcon::fromTheme(iconName).pixmap(target).toImage();
        if (size) {
            *size = image.size();
        }
        return image;
    }
};

int main(int argc, char *argv[])
{
    QGuiApplication application(argc, argv);
    QCoreApplication::setOrganizationName(QStringLiteral("Thallium"));
    QCoreApplication::setApplicationName(QStringLiteral("Thallium Store"));
    QGuiApplication::setDesktopFileName(QStringLiteral("io.thallium.Store"));

    QQmlApplicationEngine engine;
    engine.addImageProvider(QStringLiteral("theme"), new ThemeIconProvider);

    StoreClient storeClient;
    engine.rootContext()->setContextProperty(QStringLiteral("storeClient"), &storeClient);
    QObject::connect(&engine, &QQmlApplicationEngine::objectCreationFailed,
                     &application, [] { QCoreApplication::exit(1); }, Qt::QueuedConnection);

    engine.load(QUrl(QStringLiteral("qrc:/qt/qml/Thallium/Store/Main.qml")));
    if (engine.rootObjects().isEmpty()) {
        return 1;
    }

    storeClient.start();
    return application.exec();
}
