# Подсказки по публичному Rust API

## Bot API

Переданный `&tg::Bot` уже настроен на тестовый транспорт. `ChatId` и
`MessageId` создаются через `From<i64>`. Результат `Message` содержит поле
`message_id`. `send_message(chat, text)` возвращает ленивый builder с
`disable_notification(bool)`, `reply_parameters(ReplyParameters)` и
`reply_markup(InlineKeyboardMarkup)`. Для обычного ответа в текущем чате
задайте `tg::api::types::ReplyParameters::new().message_id(parent)`.
`tg::InlineKeyboardMarkup::new().row([...])` принимает кнопки, включая
`tg::InlineKeyboardButton::callback(label, data)`. Эти два builder-типа
импортируются непосредственно из `tg`.

`answer_callback_query(id).text(notification)` при await возвращает bool.
`CallbackQuery.message` может отсутствовать. Для содержимого доступного
сообщения используйте `MaybeInaccessibleMessage::accessible()`.
`Message::unknown_fields()` даёт доступ к сохранённым полям. Типы ответа
можно снова сериализовать через serde_json. `RichText::new(text).entity(range,
EntityKind::Bold)` принимает диапазон байтов UTF-8 и проверяет его границы.

За пределами коротких методов используйте `bot.api()`. Например,
`edit_message_text()` принимает `chat_id(chat.into())`, `message_id(id)` и
`text(String)`. Ответ `tg::api::types::EditMessageTextResult` различает
`Message` и `True`. `bot.api().get_me().retry_policy(policy)` принимает
`RetryPolicy::default().max_attempts(n).max_elapsed(duration)`. Параметры
`initial_delay` и `jitter` пригодятся для повторяемых проверок.
У `tg::Error` метод `outcome()` возвращает `tg::EffectOutcome`. Для тайм-аута
отправки после dispatch сохраняйте `Unknown` и исходный ключ операции.

Для `send_document(chat, file)` выберите `InputFile::bytes(name, bytes)`,
`InputFile::file_id(file.as_str())` или `InputFile::reader(name, reader, length)`.
`bot.download_file(&file).max_bytes(limit).to_writer(writer)` возвращает
фактическое число записанных байтов. После сбоя writer может содержать префикс.
`send_media_group(chat.into(), media)` принимает элементы
`tg::api::types::SendMediaGroupMediaItem`. Для документов преобразуйте
`InputMediaDocument::new("attach://part".into())` через `into()`.
`Call::attach("part", file)` связывает имя с отдельным `InputFile`.

`PollingOptions::default()` поддерживает `allowed_updates` и `max_concurrency`.
`bot.run_polling_until(router, options, stop)` управляет завершением.
`bot.webhook(router, secret)` принимает `options(WebhookOptions::default()
.max_body_bytes(limit))` и `serve_until(listener, stop)`.
`MessageContext::command_args()` возвращает `Option<&str>`. Ответ через
`MessageContext::reply(text)` переносит контекст исходного сообщения.

## Журнал и инструменты

`Request::new(profile, operation_id, ActionCall)` проверяет типизированный
вызов. Для чтения передайте None. Для записи передайте Some со стабильным
ключом. Для восстановления используйте
`ActionCall::GetOperation(GetOperationArgs { operation_id })` в теле чтения.
`Executor::execute(actor, &request, approval)` перепроверяет текущий grant.
`Grant::new(bot_id, Budget::new(label)?)` принимает `allow(Action)`,
`chats(ChatScope::only([chat]))` и `files([handle])`. `PolicyStore::replace`
привязывает grant к переданным principal и профилю. Bot ID синтетического
профиля в заданиях равен 0.

`RegisteredFile::handle()` возвращает непрозрачный идентификатор, который
используется в `SendDocumentArgs.file_handle`. `Request` преобразуется через
`into()` в `ToolRequest`. `tg_agent::tools::catalog()` и
`extended_catalog()` возвращают `ToolCatalog` без запуска хоста.
