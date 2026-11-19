# MTProto в GramRelay

`tg-mtproto` — самостоятельный клиент Telegram API для Rust. Он предоставляет
зашифрованную сессию, авторизацию бота или пользователя, типизированные
TL-вызовы, работу с сообщениями и файлами, историю и приём обновлений.
[Справочник клиента](../tg-mtproto/README.md) ·
[Выбор grammers и патчи](decisions/0016-mtproto-backend.md)

## Запустить первый пример без Telegram

Из корня полного checkout с Rust 1.89+ и подготовленным кэшем зависимостей
выполните команду. При необходимости заполните кэш через `cargo fetch --locked`.

```powershell
cargo run --locked --offline -p tg-mtproto --example offline_ping --features test-kit
```

Пример открывает временную зашифрованную сессию, проверяет байты TL-запроса
`Ping`, получает синтетический `Pong` и закрывает клиент. Он использует
тестовый обработчик и синтетический ключ. При успехе появится
`Offline MTProto Ping/Pong passed (ping_id=42).`
[Исходник примера](../tg-mtproto/examples/offline_ping.rs)

## Подключить к приложению

Путь ниже подходит приложению, расположенному рядом с полным checkout
`gramrelay`. Сессия и транспорт используют SDK из дерева проекта.
[Подключение из исходников](INSTALL-SOURCE.md)

```toml
[dependencies]
tg-mtproto = { path = "../gramrelay/tg-mtproto" }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "time"] }
```

Для живого подключения подготовьте `api_id` и `api_hash` приложения Telegram.
Храните ключ зашифрованной сессии отдельно от её файла. Для создания сессии
используйте `Session::open`. Хост выполняет вход через `sign_in_bot` либо
интерактивный вход пользователя с кодом и при необходимости 2FA. Данные
входа передаются через доверенный хост. [Как получить API ID](https://core.telegram.org/api/obtaining_api_id) ·
[Контракт авторизации GramRelay](../SPEC-mtproto-auth.md)

Для готовой сессии начните с `Session::open_existing` и
`Client::verify_account(expected)`. Хост задаёт ожидаемый идентификатор и тип
аккаунта. `Session::open_existing` ожидает зашифрованное хранилище GramRelay.
Для обычного использования сессия создаётся через вход в самом `tg-mtproto`.
Пример показывает проверку личности и закрытие клиента после вызова.
Этот вызов обращается к Telegram. [Пример с готовой сессией](../tg-mtproto/README.md#verify-an-existing-session)

Для отправки и редактирования используйте явный `Account` и полученную от него
ссылку на собеседника. При тайм-ауте с исходом `Unknown` сначала выясните статус
операции. [Пример работы с сообщением](../tg-mtproto/README.md#use-an-account-facade)

## Отправить и скачать документ

`Account::send_document` возвращает идентификатор сообщения. Передайте его и
ссылку на собеседника в `Account::document`, чтобы получить документ из
Telegram. `download_document` возвращает байты, а `download_document_to`
сохраняет их по заданному хостом пути. В обоих случаях можно указать ожидаемый
SHA-256. [Примеры отправки и скачивания](../tg-mtproto/README.md#send-and-download-documents)

## Проверить разрешённые действия агента

В этих примерах `tg-agent` работает с тестовым MTProto-клиентом, заданным
аккаунтом и явными разрешениями. Они проверяют отправку текста, callback и
документ с ключом операции.

```powershell
cargo run --locked --offline -p tg-agent --example mtproto_authorized_text --features mtproto-test-kit
cargo run --locked --offline -p tg-agent --example mtproto_authorized_callback --features mtproto-test-kit
cargo run --locked --offline -p tg-agent --example mtproto_authorized_document --features mtproto-test-kit
```

[Пример с текстом](../tg-agent/examples/mtproto_authorized_text.rs) ·
[Пример с callback](../tg-agent/examples/mtproto_authorized_callback.rs) ·
[Пример с документом](../tg-agent/examples/mtproto_authorized_document.rs) ·
[Настройка MTProto для CLI/MCP](AGENT-MTPROTO.md)

## Запустить тесты клиента

```powershell
cargo test --locked --offline -p tg-mtproto --all-targets --all-features
cargo test --locked --offline -p tg-mtproto --doc --all-features
```

Локальный набор проверяет сессии, TL-вызовы, тайм-ауты, авторизацию на
синтетических данных, операции с файлами и восстановление обновлений.
На реальном аккаунте проверены подключение, сверка личности, чтение диалогов,
отправка текста и документа, скачивание на диск и в память, редактирование
подписи и пагинация истории. Повторное скачивание прошло после переоткрытия
сессии. Результат совпал с исходным файлом побайтно.
[Результаты проверок](../tasks/todo.md)
