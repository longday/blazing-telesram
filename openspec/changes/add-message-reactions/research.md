1. **Сводка:** изменение 3 лучше разделить: базовые реакции/важность/закрепы; затем упоминания/пересылка; треды — отдельным этапом из-за отдельной модели и подписки.
2. В протоколе описаны соответствующие push-события и поля, но `yandex-proto` сейчас их не декодирует/не отправляет; существующий список записываемых вариантов закрытый.
3. В приложении уже есть ответы, отображение пересылок и признак закреплённого **чата**; это не означает поддержки пересылки как действия, закрепления сообщений или важности.
4. Главные риски — живые аккаунтные мутации и недостаточно проверенные формы запросов; нужен безопасный план ручной верификации до выпуска.
5. **Отчёт:** полный текст ниже. Записать его в `local://research-3.md` этим инструментарием не удалось: доступен только инструмент чтения, без записи в локальные артефакты.

## Отчёт: follow-up change 3

### 1. Факты протокола и подтверждённость

Основной источник — [протокольные заметки yandex-messenger-mcp](https://github.com/conarti/yandex-messenger-mcp/blob/main/docs/protocol-research.md). Это reverse-engineering заметки, а не официальная спецификация; их схемы считаю **задокументированными в исследовательском источнике**, но не подтверждёнными локально для каждого действия.

- **Транспорт мутаций.** Исходящие действия идут через Xiva WebSocket `push` с нужным объектом в `ClientMessage`; входящие события определяются присутствующим полем `ClientMessage` (источник, §§9, 9.3, 11). Локальный код уже реализует push-конверт, но только для ограниченного набора ниже (`crates/yandex-proto/src/write.rs:19-23,40-57`).
- **Треды.** `history` принимает `Threads: true`; встречаются `ThreadState` и `MentionTsMcs`. HTTP registry также описывает `join_to_thread` и `leave_thread` с `thread_id` (источник §§2.4, 9.1, 10). **Формы и семантика не верифицированы локально**; текущий `ReadRequest`/`history` интерфейс ориентирован на историю чата, не на thread-параметры.
- **Реакции.** Событие несёт `Reactions`, `RecentUserReactions`, версию, `ChatId` и timestamp. Отправка описана как `Reaction { ChatId, Timestamp, Type, Action }`, `Action` `REMOVE=1`, `REPLACE=2` (источник §§9.2–9.3). В репозитории есть fixture `crates/yandex-proto/tests/fixtures/live-push-reaction.json`, но нет реализации мутации.
- **Упоминания.** `Plain` предусматривает `MentionedUserIds`; ответ может содержать `MentionedUsers` (источник §11.1–11.2). Это доказывает наличие wire-полей в описанном протоколе, но не подтверждает, какие ID/markup/редакторные сценарии нужны для полноценного UX.
- **Пересылка.** `ForwardedMessageRefs` — ссылка на оригиналы; для пересланных сообщений ответ содержит `ForwardedMessages` с телом и `ServerMessageInfo` (источник §11.1–11.2). Точная мутация пересылки и её live-валидация в локальном проекте отсутствуют. Reply уже использует `ForwardedMessageRefs` вместе с цитатой, но не является полноценным forward.
- **Закрепление сообщения.** Событие `Pin` описано; исходящий вариант — `Pin { ChatId, Timestamp? }` (источник §§9, 9.3, 11.4). Неясно, означает ли отсутствующий timestamp переключение/снятие закрепа и каковы ограничения прав.
- **Важность сообщения.** В `Plain` есть `IsImportant`; изменение описано как `UpdateFields { ChatId, Timestamp, ImportanceFlag: 1|2 }` (источник §§9.3, 11.1). `UpdateFields` отсутствует в текущем списке разрешённых клиентских вариантов; семантика enum `ImportanceFlag` локально не подтверждена.
- **Безопасность записей.** Текущая отправка использует закрытые `WriteRequest` варианты `Plain`, `SeenMarker`, `Typing`; дизайн прямо называет реакции/пины и прочие мутации непредставимыми (`crates/yandex-proto/src/write.rs:19-23,40-57`; `openspec/changes/add-sending/design.md`, D1). Добавление возможностей должно расширять enum точечно, а не добавлять общий произвольный `push`.

### 2. Существующая поддержка и пробелы

**Уже есть**
- Ответ на сообщение: `ReplyRef`, сериализация в `ForwardedMessageRefs`/`ForwardedMessageStyles.Quote` (`crates/yandex-proto/src/write.rs:30-37,83-87`); `ReplyTarget`/`ReplyQuote` в модели (`crates/chat-core/src/model.rs:45-49`).
- Отображение связи reply/forward и метаданные `forwarded_from`: `Relation` и wire-модели (`crates/yandex-proto/src/wire.rs:98-130`); UI читает `forwarded_from` и quote в `crates/desktop/src/timeline.rs` (см. строки около 137 и 204).
- Закреплённые **чаты**, не сообщения: `ChatSummary.pinned_rank` (`crates/chat-core/src/model.rs:31-42`) и маркер в списке (`crates/desktop/src/desktop.rs`, около 436, 486).
- Обновления push сейчас классифицируют сообщения, удаления, изменения чатов, typing и `Ignored`; реакции не представлены как отдельное событие (`crates/yandex-proto/src/wire.rs:168-180`; `decode_push` — около строки 557).

**Отсутствует**
- Данные реакций и их версия в доменной модели/кэше/UI; команды и методы backend для реакции.
- Отдельные операции пересылки и закрепления/снятия закрепа; сообщения не имеют `important`, mention IDs/объектов, thread parent/state.
- Thread-scoped history, joins/leaves и отдельная навигация/подписка на треды.
- В `PushEvent` нет variants для реакции, pin, thread state, mention/importance updates. Входящие соответствующие события сейчас не превращаются в доменные изменения.
- Текущий backend предлагает лишь текст/ответ, edit/delete/seen/typing (`crates/chat-core/src/backend.rs:48-99`); команды UI ограничены теми же функциями (`crates/chat-core/src/model.rs:143-157`).
- `Message` несёт `reply_to`/`forwarded_from`, но не реакции, важность, mentions или thread-связи (`crates/chat-core/src/model.rs:82-99`).

### 3. Рекомендуемое разделение scope

1. **Reactions**: чтение/обновление с live push, доменная модель и UI, add/remove. Ограничить типы реакций тем, что подтверждено; не проектировать кастомные реакции без проверки. Хороший первый срез: есть пример live push fixture, а протокол описывает отправку.
2. **Message pins и важность**: разделить на самостоятельные capability, даже если входят в один change. Сначала входящие данные/отображение, затем явные действия с подтверждёнными формами и правами. Не смешивать с существующими pinned-chat rank.
3. **Mentions и forwarding**: связать composer/поиск участников, сериализацию `MentionedUserIds`, выбор оригинала и отображение источника. Для forwarding сначала определить UX: пересылать сообщение без текста-комментария, с комментарием или оба варианта; reply-механизм не считать достаточной реализацией.
4. **Threads** — отдельное крупное изменение: thread-aware история, кэш-ключи/связи, навигация UI, join/leave, live state, mentions внутри треда. Источник описывает несколько API/режимов; это больше, чем добавление одного поля.

Если необходим один релизный change, разумно включить только reactions + display/read support для важности/закрепления, а отправку pins/importance отложить до проверки. Не включать полноценные threads в тот же change.

### 4. Открытые решения пользователя

- **Должны ли закреп/важность быть только видимыми или управляемыми?** Рекомендация: сначала видимость входящих состояний, затем отдельные действия после проверки прав и форм. Компромисс: UX откладывается, зато снижается риск ошибочной мутации.
- **Какой UX forwarding?** Рекомендация: явная команда «Переслать» с выбором чата и предварительным просмотром; комментарий — отдельный необязательный текст. Компромисс: сложнее, чем просто копировать ссылку, зато соответствует пересылке между чатами.
- **Треды — новые сущности/вкладки или просто фильтр истории?** Рекомендация: модель/навигация как отдельный thread context, не «режим истории» без модели. Компромисс: больше архитектурной работы, но не смешивает сообщения разных контекстов в кеше.
- **Упоминания должны быть только вводом/подсказкой или также поиском/фильтром?** Рекомендация: выбрать ввод + корректный показ в первом срезе; отложить поиск. Источник подтверждает `MentionedUserIds`, но не полный UX-контракт.
- **Можно ли проверять мутации на реальном аккаунте?** Рекомендация: только после явного согласования целевого чата/сообщения и обратимого тестового сценария. Пока — fixtures и read-only исследование.

### 5. Риски

- Все мутации изменяют состояние аккаунта. Удаление, пересылка, pin/importance и реакции могут быть видны другим участникам; снятие/изменение закрепа может быть трудно восстановить.
- Схемы взяты из reverse-engineering, не из гарантированного публичного API. Неизвестные enum-коды/права чата могут приводить к отказу или неправильному действию.
- Для тредов ошибочное связывание history/cache может показать сообщения не в том контексте или потерять их из обычной истории.
- Существующая архитектура намеренно не допускает произвольные мутации; обход closed write set ради ускорения существенно увеличит поверхность риска.
- Упоминания требуют корректной идентичности пользователя и форматирования, иначе текст может отображаться правильно локально, но не уведомлять адресата.

### Файлы

- `openspec/changes/archive/2026-11-23-add-read-only-client/proposal.md:17-25` — объявленный порядок будущих возможностей.
- `openspec/changes/add-sending/design.md` — закрытый набор записей, имеющиеся reply/push формы, явные non-goals reactions/pins.
- `openspec/changes/add-sending/specs/message-sending/spec.md` — пользовательский write-контракт и запрет прочих мутаций.
- `crates/yandex-proto/src/write.rs:19-57,83-87` — текущие разрешённые write-варианты и encoding ответа.
- `crates/yandex-proto/src/wire.rs:98-180,557` — wire-модель связей и текущие push variants/декодер.
- `crates/yandex-proto/tests/fixtures/live-push-reaction.json` — пример reaction push.
- `crates/chat-core/src/model.rs:31-49,82-99,143-157` — текущие поля модели и команды.
- `crates/chat-core/src/backend.rs:30-99` — live events и backend seam.
- `crates/desktop/src/timeline.rs` (около 137, 204) — показ forwarding/reply-метаданных.
- `crates/desktop/src/desktop.rs` (около 436, 486) — pinned-chat UI, не message pins.

## Reaction types

Task 1.1, read-only, 2026-11-24. No account was used and nothing was written: everything below comes from public static files and an anonymous page request.

### Sources

- `https://yandex.ru/chat` (anonymous GET) names the current build: `https://yastatic.net/s3/chat-static/telemessenger/_/3.6.3/web/app.js` (4 078 115 bytes, sha256 `88292adf0e94312b84f24ccc91d0556519d85f7eea635b6016a12b41adb0bc73`). The page also embeds the server config (`reactions_enabled: true`, `reactions_by_namespace`, `extended_reactions_by_namespace`).
- The build the protocol notes name (`.../chat-static/_/0x8b21684/web/app.js`, `chats-web/3.21.0`, last modified 2026-07-08) is still served but is older and has **no** table: it contains no code `1001xx`, and its embedded default config has `reactions_enabled: false` with empty namespace maps. It is not used as evidence; it only confirms `ReactionAction` (`REMOVE = 1`, `REPLACE = 2`).
- Where one pair is found in the current build (the "locations" cited in the evidence column):
  - **E**: module `70109`, enum `EmotionCode`: code to name (`CHECK = 100101`, ...). Legacy codes are Unicode code points (`UNICODE_LIKE = 128077`).
  - **U**: module `35031`, `getReactionImageUrl`: code (via enum name) to image slug, e.g. `CHECK` to `2358365/reaction-check-ext`. The web client draws reactions as server-hosted images, **not** as emoji text; no code-to-emoji-character table exists in the bundle. The emoji here is the Unicode character the enum name designates, checked against the artwork.
  - **C**: the server config embedded in the `/chat` page: the picker lists, by chat namespace.
  - **I**: the image itself. `https://files.messenger.yandex.net/reactions/{code}/small` and `https://avatars.mds.yandex.net/get-mssngr/{slug}/orig` return byte-identical PNGs (512x512) for every code `100101`-`100157`, so the E-to-U link is checked from the server side; each image was looked at.
  - **L**: legacy twin: a pre-`1001xx` code whose value is the Unicode code point and whose image is the earlier artwork of the same reaction.
  - **F**: `crates/yandex-proto/tests/fixtures/live-push-reaction.json`: a live `Reaction` push whose `Reaction.Type`, `Reactions[].Type` and `RecentUserReactions[].Type` are all `100101`. It confirms that `100101` is a code real traffic carries and counts; it says nothing about the glyph.
- A self-chat observation (a second source of a different kind) was not possible: no signed-in session and no account writes in this task. Task 7.2 may add one.

### Confirmed pairs, in picker order

Order is the server's `default` / `private` list from C, `[100109, 100108, 100104, 100102, 100103, 100106, 100105, 100110, 100101]`: the web client's default picker for a private chat and for a group chat with no namespace entry. It is not the order of the extended list.

| # | code | enum name | emoji (code points) | evidence |
|---|------|-----------|---------------------|----------|
| 1 | 100109 | `HEART` | ❤️ (U+2764 U+FE0F) | E, U, C, I; L: `UNICODE_HEART = 10084` (U+2764) |
| 2 | 100108 | `SMILE` | 😆 (U+1F606) | E, U (`reaction-laugh-ext`), C, I; L: `UNICODE_SMILE = 128518` (U+1F606, slug `reaction-laugh`). The new artwork is a grin with arc eyes (closer to 😄); the legacy code fixes 😆 |
| 3 | 100104 | `FIRE` | 🔥 (U+1F525) | E, U (`reaction-flame-ext`), C, I; L: `UNICODE_FIRE = 128293` |
| 4 | 100102 | `LIKE` | 👍 (U+1F44D) | E, U, C, I; L: `UNICODE_LIKE = 128077` |
| 5 | 100103 | `DISLIKE` | 👎 (U+1F44E) | E, U, C, I; L: `UNICODE_DISLIKE = 128078` |
| 6 | 100106 | `SURPRISED` | 😲 (U+1F632) | E, U, C, I; L: `UNICODE_SURPRISED = 128562` |
| 7 | 100105 | `CRYING` | 😭 (U+1F62D) | E, U, C, I; L: `UNICODE_CRYING = 128557` |
| 8 | 100110 | `EYES` | 👀 (U+1F440) | E, U (`reaction-eyes-ext`), C, I; no legacy twin; the name designates 👀 and the artwork is a pair of eyes |
| 9 | 100101 | `CHECK` | ✅ (U+2705) | E, U (`reaction-check-ext`), C, I, **F**; no legacy twin; the artwork is a white tick on a green disc, which is ✅ (✔️ and ☑️ rejected: not green) |

`KNOWN` (task 3.3) is these nine rows, in this order. Every code is also in the `extended_reactions_by_namespace.private` list, so a server that offers the extended picker accepts them too.

### Not in the table

- Namespace 17 (`reactions_by_namespace["17"]`) offers only `100102` and `100103`; a group chat of that namespace may refuse the others (the server answer is an error that rolls the toggle back, D5). The client does not know the namespace, so the picker stays the nine.
- `100107` `GOOSE` is in `default_team` only. Its legacy twin `UNICODE_GOOSE = 129442` is U+1F9A2 (a swan, 🦢) while the artwork is a goose: the enum's own code point is a placeholder, so no Unicode glyph is confirmed. It stays out; a chip for it shows `NEUTRAL_GLYPH`.
- Codes `100111`-`100157` (extended picker, flag `web_enable_extended_reactions_picker`, `false` in the page config) and the seasonal `1000xx` / `110001` codes are not offered by the default picker. Several have an unambiguous Unicode name (💯 `HUNDRED_POINTS`, 🤡 `CLOWN`, 💩 `POOP`, 🤯 `EXPLODING_HEAD`, 💔 `BROKEN_HEART`, 🎉 `TADA`, 👏 `CLAP`, 🤔 `THINKING`, 😎 `SUNGLASSES`, 🙏 `PRAY`, 🤝 `HANDSHAKE`, 👌 `OK`), others are product art with no Unicode counterpart (`PLUS`, `MINUS`, `FOUR_PLUS`, `LOGO_360`, `ROWAN`, `CALENDAR`, `PIXEL_HEART`, `FLOWERS_*`). None has a second source beyond E, U, I, so none is confirmed; they render as `NEUTRAL_GLYPH` chips. A follow-up change that adds the extended picker can use this list as its starting point.

### Other facts found in the bundle (not for the table)

- Picker contents are server config, not constants: `reactions_enabled` (`false` hides reactions entirely) and per-namespace lists. Task 3.3 transcribes the private/default list; a later change could read the config instead.
- Wire form of the web client (both builds): `sendReaction(chat, timestamp, type)` pushes `Reaction { ChatId, Timestamp, Type, Action: 2 }` and `removeReaction(chat, timestamp, type)` pushes `Reaction { ChatId, Timestamp, Type, Action: 1 }`: **the web client's `Remove` carries `Type`** (its method takes the type as a parameter; the callers are not recoverable from the minified bundle, so what they pass is inferred to be the user's current reaction). D2 sends `Remove` without `Type` (the notes say the server demands `Type` only for add and replace); this is unverified, and task 7.2 must test the bare form first and fall back to `Type: <current code>` if refused, as D2 already says.
- Chip order in the web client's old build (3.21.0, `reactions` selector): count descending, ties by type descending. D3 keeps server order and appends a new type last; if the server's order turns out to differ from the web client's, that is a display difference, not a data one.