# Первый бот: команда и кнопка

Нужны установленный Rust 1.89+ и полный [checkout либо source ZIP](INSTALL-SOURCE.md).
Код alpha закрыт; условия описаны в [закрытой поставке](PRIVATE-DISTRIBUTION.md).
Если зависимости ещё не загружены, выполните из корня `cargo fetch --locked`.

## Проверить обработчики без Telegram

```powershell
cargo run --locked --offline -p tg --example command_callback --features test-kit
```

Ожидаемый вывод: `Offline command and callback passed: two requests, no sockets or real credentials.`
Режим по умолчанию проверяет `/start`, отправленную кнопку, точный callback и
его подтверждение в памяти. Настоящий токен не нужен; переменная токена не читается.
Исходник — [command_callback.rs](../examples/command_callback.rs).

В его `router()` два обработчика:

```rust
Router::new()
    .command("start", |ctx: MessageContext| async move {
        let keyboard = InlineKeyboardMarkup::new()
            .row([InlineKeyboardButton::callback("Confirm", "confirm")]);
        ctx.reply("Press the button").reply_markup(keyboard).await?;
        Ok(())
    })
    .callback("confirm", |ctx: CallbackContext| async move {
        ctx.answer().text("Button received").await?;
        Ok(())
    })
```

Это фрагмент функции из примера; импорты и main берите из полного файла.
`reply()` сохраняет контекст исходного сообщения. `answer()` подтверждает
нажатие, включая inline callback без доступного сообщения. Значение callback
не даёт пользователю права на прикладное действие: такую проверку добавляет хост.

## Запустить своего бота

Подготовьте `TELEGRAM_BOT_TOKEN` в окружении процесса через секрет-хранилище или
настройки хоста. Не вставляйте токен в исходник, аргументы запуска или журнал.
Для одного бота используйте один polling-процесс; webhook должен быть отключён
осознанно владельцем. Пример не удаляет webhook и не сбрасывает updates.

Следующая команда выполняет реальные запросы и предназначена для явного запуска
в вашем подготовленном окружении:

```powershell
cargo run --locked --offline -p tg --example command_callback --features test-kit -- --live
```

Откройте диалог с ботом, отправьте `/start` и нажмите **Confirm**. Появится
уведомление **Button received**. Работают те же обработчики, что в offline-проверке.
Polling по умолчанию подписан на `message` и `callback_query`. Ctrl+C запускает
штатную остановку; дождитесь завершения процесса.

Меняйте тексты и маршруты в `router()`, затем повторяйте offline-проверку и:

```powershell
cargo test --locked -p tg --example command_callback --features test-kit
```

## Подключить к приложению

Для собственного приложения подключите `tg` по path из полного дерева, как в
[инструкции](INSTALL-SOURCE.md). `bot` включён по умолчанию; `test-kit` добавляет
in-memory тестовый транспорт, `webhook` — HTTP-адаптер. SQLite и MCP находятся
в отдельном `tg-agent` и не нужны обычному Bot API-клиенту.

Дальше: [файлы, ошибки и устойчивость](RECIPES.md),
[указатель для агента](AGENT-INDEX.md), [миграции](MIGRATION.md).
Цель первого запуска за 15 минут остаётся критерием приёмки: локальная проверка
примера не заменяет наблюдение нового пользователя с реальным ботом.
