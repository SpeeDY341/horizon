# План переделки DLSS, Reflex и Frame Generation

Дата: 31 октября 2026 года. Обновлено 1 ноября. **Код A–H принят независимыми
review после исправлений, включая review_g2 и review_h3 PASS. Часть I обновляет
документацию и сводит итоговые проверки; её review ещё требуется. Настоящий FG 2×,
полная аппаратная lifecycle/визуальная приёмка и сборки Linux/macOS открыты.**
Итоговые результаты и команды: [dlss-validation.md](dlss-validation.md).

Повторный полный review `8b33c7954` открыл F1–F5. Для F1 добавлен
creation path DX12 и настоящий SDK-тест буферов/resize/recreate; свежий
независимый review исправления ещё требуется. Прежние PASS не означают
полной приёмки интеграции; активная FG 2× остаётся недоказанной.

Основание: review 30 коммитов `763528025..35123b73f` личного форка
`SpeeDY341/horizon`, ветка `codex/dlss-stages-0-2`.
Полный локальный отчёт находится в `.local/fork-review-2026-10-31.md`,
лог проверок — в `.local/review-pr-ready.log`. Эти локальные файлы не публикуются.

## 1. Цель и границы работы

Исправить девять замечаний review и проверить работоспособность итоговой
интеграции. Основная переделка — единое управление кадром, который будет показан:
его token, константами камеры, входными изображениями и состоянием swapchain.

- [ ] Восстановить сборку Windows, Linux и macOS.
- [ ] Исправить подключение Frame Generation к Streamline.
- [ ] Согласовать Super Resolution (SR), Reflex, FG и запасной TAA.
- [ ] Сделать смену размера и восстановление поверхности безопасными.
- [ ] Исправить атрибуцию, изоляцию тестов и документацию.
- [ ] Подтвердить результат тестами, настоящим Present и визуальной проверкой.

Все NVIDIA-функции остаются выключенными по умолчанию. Симуляция работает
с частотой настоящих кадров/тиков; сгенерированные кадры не вызывают игровую
логику. RTX-тени и Ray Reconstruction в этот план не входят.

Создание issues, комментариев и PR, push и переписывание истории не являются
частью выполнения этого документа. При подготовке публикации отдельно применить
правила AGENTS.md: проверка чужих PR, claim, маркировка агента и атрибуция.

## 2. Соответствие замечаниям review

| № | Приоритет | Проблема | Этап исправления |
| --- | --- | --- | --- |
| R1 | P1 | Сборка вне Windows | 4 |
| R2 | P1 | Нет флага frame-based resource tagging | 7 |
| R3 | P1 | Отключение FG происходит после resize | 5 |
| R4 | P2 | Разные constants на одном frame token | 6 |
| R5 | P2 | FG читает discarded depth | 6 |
| R6 | P2 | DX12 hook не восстанавливается после surface loss | 5 |
| R7 | P2 | Не сохранены notices новых ABI | 3 |
| R8 | P2 | Тесты используют постоянный общий home | 3 |
| R9 | P3 | Документация противоречит реализации | 9 |

## 3. Подготовить воспроизводимую и изолированную проверку

**Файлы:** `tools/play_dlss_test.ps1`, `tools/Play-DLSS-Test.cmd`,
`crates/horizon-streamline/src/{reflex,frame_gen}.rs`, `THIRD_PARTY_NOTICES.md`.

- [x] Зафиксировать SHA проверяемой версии, версии SDK/драйвера, GPU и API.
- [x] Оставить существующий `conversation.md` вне изменений и проверяемого diff.
- [x] Создавать уникальную scratch-папку на каждый тестовый запуск.
- [x] Направлять в неё `HORIZON_HOME`, `HORIZON_LAUNCHER_HOME`, `XDG_DATA_HOME`,
  `XDG_CONFIG_HOME`, а также launcher trash/clipboard/desktop.
- [x] Устанавливать `HORIZON_LAUNCHER_NO_SELF_UPDATE=1` и `HORIZON_NO_AUDIO=1`.
- [x] Разнести сохранения, отчёты и логи параллельных запусков.
- [x] Если для перезапуска нужен выбранный API, переносить только явно выбранные
  настройки в новый scratch; не переиспользовать каталог мира.
- [ ] Проверить обычный запуск на title screen и отдельный режим `-Smoke`.
- [x] Проверить, что два одновременных запуска используют разные пути и файлы.
- [x] Сверить LICENSE и заголовки использованных файлов NVIDIA на закреплённом
  commit; сохранить исходные copyright/permission notices у новых ABI-переводов.
- [x] Дополнить `THIRD_PARTY_NOTICES.md` файлами Reflex/FG и точными источниками.

**Готово, когда:** тестовые запуски не наследуют мир и не конфликтуют за логи;
адаптированные декларации имеют требуемую атрибуцию. SDK и игровые ресурсы
остаются только в игнорируемых локальных каталогах.

### Результат части A (31 октября 2026)

- База проверяемых изменений: `35123b73f1ed4f9ef10508bc4f57b88d1854d7de`.
  Официальный локальный Streamline `sl.interposer.dll`: версия 2.14.16.0.
  Публичные заголовки и `license.txt` сверены на закреплённом commit
  `625db07d4bca5abc67d781c42af7e7fb0761f1b6`; notices Reflex/PCL/DLSS-G
  сохранены в адаптированных файлах с точными именами источников.
- Каждый запуск получает `horizon-dlss-test-<GUID>` под системным TEMP.
  После смены API можно явно перенести только графические настройки:
  `tools\Play-DLSS-Test.cmd -GraphicsSettingsFrom "<предыдущий scratch>\horizon-graphics.txt"`.
  Миры, options.txt и логи при этом создаются заново; `-Smoke` передаётся через cmd.
- `python -m unittest discover -s tests -p test_play_dlss_test.py -v`: 3 passed.
  Тесты запускают настоящий скрипт в Windows PowerShell 5.1, подменяя только
  Start-Process: проверены два параллельных вызова (title/Smoke), все защитные
  env, отдельные логи/путь отчёта, перенос одного файла без saves/options и
  отказ при отсутствующем файле настроек. Фикстуры используют только temp.
- Парсер Windows PowerShell 5.1, `cargo fmt --all -- --check`,
  `cargo test -p horizon-streamline --lib` (7 passed, 3 ignored),
  `git diff --check`, `python tools/check_public_tree.py`: успешно.
  Cargo сообщил о fallback с hard links на копирование incremental cache.
- В ограниченной среде TEMP/TMP для unit-прогона направлены в
  `.local/dlss/test-tmp`: системный sandbox TEMP не позволял создавать
  вложенные папки. Сам скрипт не требует фиксированного пути TEMP.
- Настоящие title/Smoke-запуски, GPU/драйвер и API аппаратного прогона здесь
  не проверялись. Изоляция доказана тестом запуска, работоспособность FG/Reflex
  из него не следует. Полный pr_ready и аппаратные SDK-тесты не запускались.
  Независимый агент `review_a` проверил commit
  `1f375ead463d6e72818a6b5e8164c3d62fb3cc66` и дал **PASS** в этом чате.
  Review дополнительно подтвердил передачу реальных аргументов Start-Process
  через helper exe в PowerShell 5.1. Остальные замечания остаются открытыми.

## 4. Восстановить платформенные границы — R1

**Файлы:** `crates/horizon-client-render/Cargo.toml`,
`crates/horizon-client-render/src/gfx.rs`, `crates/horizon-streamline/src/lib.rs`.

- [x] Сделать зависимость `horizon-streamline` общей, используя существующие
  заглушки вне Windows; нативные зависимости и реализации оставить под cfg.
- [x] Проверить все публичные функции, типы и re-export на наличие Windows-only
  ссылок в общем коде, включая `reflex_server_tick`.
- [x] Проверить статически, что Linux/macOS не пытаются загружать NVIDIA DLL
  и получают корректный статус недоступной функции.
- [ ] Выполнить проверки рендерера и клиента на Windows, Linux и macOS.
- [x] Проверить dedicated/server-конфигурацию с `--no-default-features` на Windows.
- [x] Закрепить нужные проверки в существующей CI-матрице, если они отсутствуют.

**Готово, когда:** общий код собирается на всех трёх ОС. Отсутствие локального
target или доступа к нужной ОС фиксируется как непроведённая проверка,
а не успешный результат.

### Результат реализации части B (31 октября 2026)

- `horizon-streamline` — общая обязательная зависимость рендерера, поэтому
  `gfx::reflex_server_tick` разрешает тип и функцию на любой платформе.
  Runtime, ABI, SR/FG/Reflex native API и их re-export остаются под Windows cfg;
  `libloading`, `ash`, `wgpu` и `pollster` также подключаются только на Windows.
  На других ОС доступны только `eligible` (false) и no-op guard server tick.
- Проверены все ссылки `horizon_streamline` в renderer/client: вне cfg остался
  только общий guard. `dlss::load` вне Windows возвращает None до загрузки DLL;
  fallback SR — `TAA: Windows`, Reflex/FG — `unavailable` вместо `restart`.
  Для non-Windows добавлены Rust-тесты отсутствия сессии, fallback и guard.
- На Windows успешно: `cargo check --locked -p horizon-client-render -p
  horizon-client --lib --bins`; `cargo check --locked -p horizon-client -p
  horizon-server --no-default-features --lib --bins`; `cargo clippy --locked -p
  horizon-client-render -p horizon-streamline --lib -- -D warnings`;
  `cargo fmt --all -- --check`.
- `cargo test --locked -p horizon-streamline --lib`: 7 passed, 3 SDK-теста ignored.
  `cargo test --locked -p horizon-client-render --lib gfx::dlss::tests::`: 3 passed.
  `python -m unittest discover -s tests -p test_streamline_platform.py -v`:
  2 passed; тесты фиксируют общую зависимость и Windows-only native dependencies.
  `git diff --check` и `python tools/check_public_tree.py`: успешно.
  Cargo предупреждает о копировании incremental cache вместо hard links.
- `.github/workflows/ci.yml` уже запускает Linux workspace Clippy/tests и
  `horizon-client --no-default-features`. `game-build.yml` уже содержит сборки
  клиента/server на Linux, Windows и macOS, плюс отдельный Linux server build
  без default features. Расширять матрицу не потребовалось; её выполнение
  для этого коммита здесь не запускалось.
- Установлен только `x86_64-pc-windows-msvc`. Linux/macOS builds и новые
  non-Windows Rust-тесты локально **не выполнялись**. Manifest-тесты и cfg-аудит
  не заменяют эти сборки, поэтому общий критерий готовности части B и пункт
  проверки всех трёх ОС остаются открытыми. Игра, launcher и SDK GPU-тесты
  не запускались; полный pr_ready не запускался. `Cargo.lock` не изменился.
  Независимый агент `review_b` проверил commit
  `d1b3a048d71edd404fd50eb131b07dcf76b6078d` и дал **PASS** в этом чате.

## 5. Переделать жизненный цикл swapchain — R3, R6

**Файлы:** `gfx.rs`, `horizon-streamline/src/{runtime,frame_gen}.rs`,
при необходимости существующие точки расширения `third_party/wgpu-hal`.

- [x] Найти все пути создания, configure, resize, замены и явного release surface,
  включая present mode, HDR, fullscreen, resource reload и surface loss.
- [x] Ввести общий путь подготовки к изменению swapchain: отправить FG Off
  **до** операции, проверить результат и выполнить требуемую синхронизацию.
- [x] Не продолжать потенциально опасную реконфигурацию после ошибки отключения
  FG без определённого безопасного пути восстановления.
- [x] После создания нового DX12 swapchain устанавливать Streamline proxy до
  первого Present; учитывать замену native pointer и владение COM-ссылками.
- [x] При ошибке установки hook переводить presentation-зависимые функции
  в недоступное состояние с понятной причиной.
- [x] После восстановления сбрасывать историю и возобновлять FG только на кадре
  с актуальными ресурсами, размерами, token и константами.
- [x] На pause/title/menu/focus loss прекращать генерацию; на нулевом размере
  не пытаться показывать кадр или сохранять активный режим FG.
- [x] Добавить тест последовательности вызовов с подменённой границей SDK:
  `FG Off → изменение surface → установка hook → валидный кадр → FG On`.
- [ ] Проверить повторные resize, Alt+Tab, minimize/restore, fullscreen и
  принудительное восстановление **новой** surface на обоих API.

**Готово, когда:** FG Off всегда предшествует реконфигурации; новый DX12
swapchain получает hook; стресс-проверка не зависает и не требует перезапуска.

### Результат кода части C (31 октября 2026)

- База: `d1b3a048d71edd404fd50eb131b07dcf76b6078d`. Прочитаны MIT license,
  `sl_dlss_g.h` и ProgrammingGuideDLSS_G, раздел 12.0, SDK 2.14.16 на commit
  `625db07d4bca5abc67d781c42af7e7fb0761f1b6`. Off не требует frame token;
  размеры/форматы в options optional. Используется default
  `eBlockPresentingClientQueue`; после успешного Off ожидается presenting queue
  через device.poll с таймаутом 5 секунд. Проверки подписей не менялись.
- Общая подготовка вызывается перед resize (в том числе 0×0), present mode,
  HDR, recreate, acquire retries, reconfigure после suboptimal Present,
  fullscreen в игре/title и явной передачей surface при resource reload.
  Renderer::new с прежним renderer проверяет Off до получения spare surface;
  у первого нового runtime FG ещё не bound и не мог быть включён.
- Ошибка Off/queue wait останавливает mutation; текущий renderer прекращает
  рисование. release_surface возвращает ошибку без передачи владения, reload
  её распространяет. Fullscreen сохраняет запрос для повторной попытки.
- Сбрасываются SR/TAA/motion history и состояние FG. Режим пользователя
  сохраняется; Session не включает FG до кадра с reset и существующими
  depth/motion/constants/token. Нулевой размер отключает SDK немедленно,
  не ожидая record следующего кадра. Старый delayed size-check оставлен
  дополнительной защитой, а не механизмом lifecycle.
- Регрессии: native mock slDLSSGSetOptions проверяет Off даже после failure
  session, результат ошибки, повтор и сохранение wanted; injectable SDK/queue
  boundary проверяет Off → wait → mutation и short circuit обеих ошибок.
  Fullscreen-тест проверяет отказ без window mutation, успешный повтор и
  no-op без обращения к SDK. Это unit-проверки, не настоящие Present/resize.
- На Windows прошли targeted client check, Clippy lib/bins/tests четырёх
  затронутых crates с `-D warnings`, Streamline unit tests (8 passed,
  3 SDK ignored), renderer lifecycle (2 passed), renderer DLSS (3 passed),
  input window tests (12 passed). Cargo предупреждает о fallback incremental
  hard links на копирование. Полный pr_ready не запускался.
- Аппаратный прогон игры/launcher, FG stress resize/minimize/fullscreen,
  Linux/macOS и независимый review C ещё не выполнены. R2/R4–R6 и часть D
  остаются открытыми; установка proxy для новой surface здесь не добавлялась.
  Shutdown/destructor не превращён в новый путь восстановления/утечек;
  покрыты перечисленные явные операции lifecycle. `conversation.md` не тронут.

### Исправление по review части C

- Независимый `review_c` дал **FAIL** для
  `9628c314ee9b4e41859f666be21986362e7b016e`: reset первого кадра после
  suspend мог пройти под меню с FG allow=false. Следующие кадры теряли reset,
  а gate Session ожидал его бесконечно и не возобновлял FG после закрытия меню.
- Renderer теперь читает pending reset Session перед jitter и формированием
  SR/FG constants. На первом подходящем кадре сбрасываются camera/motion
  history; paused-кадры не потребляют pending и не заставляют TAA постоянно
  сбрасываться. SR и FG получают reset из раннего состояния кадра; позднее
  изменение констант только внутри FG не добавлялось. Объединение immutable
  common constants и существующая проблема R4 остаются задачей E.
- Pending снимается только после успешного SDK On. Ошибка options сохраняет
  его для повтора; отключённая после failure session не сбрасывает историю
  обычного рендера. Условия допустимости кадра вынесены в общий predicate
  для ранней подготовки и позднего record.
- Native mock регрессия проверяет suspend → три paused кадра с reset
  true/false/false → ошибку On → успешный On с reset → обычный кадр без reset.
  Headless wgpu-тест проверяет ту же последовательность temporal camera при
  SR Off: первый разрешённый кадр получает новый reset, следующий — уже нет.
  Это не тест настоящего FG Present: SDK boundary подменён.
- Targeted проверки Windows: Streamline 9 passed/3 ignored; renderer lifecycle
  3 passed (включая headless wgpu); Clippy renderer/Streamline/client
  lib/bins/tests с `-D warnings` успешно. Повторный независимый review fix
  ещё требуется; аппаратный FG stress и Linux/macOS не выполнялись.

### Восстановление после временной ошибки Off по review C2

- `review_c2` дал **FAIL** для C после `49a50de7d`: исправление pending reset
  прошло, но resize терял требуемый размер при ошибке Off/wait. Флаг ошибки
  затем бессрочно пропускал draw до внешнего изменения окна или настроек.
- Флаг заменён конкретной очередью последних запросов resize, present mode,
  HDR и fullscreen. После отказа намерение остаётся в очереди; render повторяет
  подготовку до проверки suspended, без нового события окна. Очередь хранит
  только последний запрос каждого вида и не сбрасывает задержку при новых
  событиях. Fullscreen передаёт владение запросом от input renderer-у;
  параллельного второго pending-запроса в input после принятия нет.
- Максимум три попытки, задержки 250 и 500 мс; при ожидании повторного Off
  поверхность не меняется и новый кадр не рисуется. Первая ошибка логируется
  один раз. Третий отказ возвращается из render как явная ошибка, которую
  существующий game error path показывает пользователю. Loading/no-resources
  сохраняют terminal render error вместо бесконечных предупреждений.
- Перед release очередь должна завершиться; pending recovery возвращает
  явную ошибку reload без передачи surface. No-op present/HDR не выполняют
  лишнюю реконфигурацию. Никакой небезопасный fallback после ошибки Off не добавлен.
- Три теста выполняют тот же Pending::poll, который вызывается renderer-ом,
  с injectable SDK/mutation и управляемым временем: отказ → задержка → успешный
  Off → сохранённый resize → разрешение рендера без внешнего события;
  повторные отказы/новые размеры не обходят backoff и предел попыток;
  present/HDR/fullscreen сохраняются и coalesce до последнего запроса.
- На Windows прошли recovery 3 tests, прежние lifecycle 3 tests (с headless
  wgpu), input window 12 tests, Streamline 9 tests/3 ignored; Clippy четырёх
  затронутых crates lib/bins/tests с `-D warnings`. Повторный независимый
  review и аппаратный FG стресс остаются открытыми; D не менялась.

### Сохранение порядка очереди по review C3

- `review_c3` дал **FAIL** для `c7d2b7a4f`: coalescing перемещал повторный
  resize за no-op present mode. Успешный no-op сбрасывал failures, позволяя
  drag-событиям бессрочно обходить предел трёх попыток.
- Запрос того же вида теперь заменяется на исходной позиции очереди. Ошибка
  в её голове не пропускает поздние no-op и сохраняет failures/backoff.
  Регрессия воспроизводит Loading resize → apply_menu_settings с повторными
  Resize/PresentMode: ровно три Off-отказа, затем terminal Err; no-op не
  вызывается и не обнуляет счётчик. Missing также выходит через существующий
  error path после terminal render error, без новой серии redraw.
- Targeted recovery tests: 4 passed; Clippy renderer/client lib/bins/tests с
  `-D warnings`, fmt, diff check и public-tree успешно. Повторный независимый
  review fix и аппаратные проверки остаются открытыми.

### Принятие кода C и результат кода D (31 октября 2026)

- Независимый `review_c4` дал **PASS** для исправлений C на
  `5b967731778976a71e2e43675f26d212b5af0978`. Аппаратная проверка C
  остаётся открытой; этот PASS не подтверждает настоящий FG Present.
- D основана на этом SHA. Все surface configure проходят общий
  `Renderer::configure`: первый new, new(previous), resize, present mode,
  HDR, acquire/reconfigure recovery и recreate_surface с новой или spare
  surface. После configure hook вызывается синхронно до первого acquire/Present;
  существующие C Off/wait guards перед mutation сохранены.
- HAL хранит факт установки proxy на самом owned SwapChain и переносит его
  через ResizeBuffers. Новая цепочка начинает без proxy даже при повторном
  использовании адреса старой. Runtime больше не распознаёт proxy по голому
  адресу. Переход на новый proxy передаёт HAL одну owned COM-ссылку; старый
  интерфейс освобождается прежним HAL-путём, дополнительных удержаний нет.
- Ошибка `slUpgradeInterface` (код/нулевой результат либо отсутствующий chain)
  сохраняется как presentation unavailable. SR использует TAA, FG не пишет
  tags/options для кадра, Reflex не выделяет/sleep новый token; открытый и
  pending tokens сбрасываются. Статусы содержат причину. Обычный native Present
  разрешён после успешных C Off/wait. Следующий успешный configure снимает
  presentation error; SDK не выгружается, пока его объекты ещё могут жить.
  Vulkan, no-SDK и offscreen без попытки hook не требуют DX12 proxy.
- Проверены закреплённые MIT license и `include/sl_core_api.h` Streamline
  2.14.16, commit `625db07d4bca5abc67d781c42af7e7fb0761f1b6`:
  manual upgrade выполняется после создания интерфейса и до presentation.
  Новых переводов ABI/чужого исходного кода не добавлено.
- Windows: Streamline lib **12 passed, 3 SDK ignored**; renderer lifecycle
  **5 passed** (включая injected new surface → configure → hook → Present
  и hook failure → native Present). Mock SDK проверяет код ошибки, null-success,
  восстановление доступности; Reflex проверяет сброс обоих видов tokens.
  HAL отдельно: `cargo test --manifest-path third_party/wgpu-hal/Cargo.toml
  --no-default-features --features dx12 --lib proxy_tests --target-dir target`
  — **2 passed**, idempotent proxy/ошибка/повтор и новый chain по старому адресу.
  Для standalone dev dependencies потребовалась загрузка crates.io;
  сгенерированный отдельный Cargo.lock не включён в изменение.
- Clippy streamline/renderer/client lib/bins/tests с `-D warnings`, fmt
  (workspace и отдельный HAL), diff check и public-tree проходят. Cargo
  предупреждает о fallback incremental hard links на копирование.
  Полный pr_ready, аппаратные loss/recreate/FG Present, визуальная проверка,
  Linux/macOS и независимый review D **не выполнены**. D не закрывает R2/R4/R5;
  проверка настоящего возобновления FG остаётся зависима от E–G.

### Исправление по review D

- `review_d` дал **FAIL** для `98eb2d9b77cbf4b2225bb25c9641fd27333fcaf8`:
  при отказе hook Session сохраняла pending reset, а renderer применял его
  на каждом fallback-кадре мира. Из-за этого запасная TAA постоянно теряла
  историю, пока FG оставалась недоступна.
- Production decision `prepare_frame_history` теперь требует готовый
  presentation path, который renderer читает через Runtime. Отказ hook или
  poisoned lock не разрешает применять reset к TAA. Pending в Session
  сохраняется для первого подходящего кадра после успешного hook.
- Новая регрессия выполняет настоящий Post с TAA On и движущейся камерой:
  четыре hook-failed fallback-кадра дают reset `1, 0, 0, 0`; первый кадр
  после восстановления hook даёт `1`, следующий после успешного FG — `0`.
  Она вызывает тот же production helper, что renderer. Mock SDK/runtime
  дополнительно проверяет, что pending переживает четыре чтения unavailable
  и остаётся установленным после успешного upgrade новой цепочки.
- Windows: lifecycle **6 passed**, Streamline **12 passed, 3 ignored**.
  Clippy streamline/renderer/client lib/bins/tests с `-D warnings`, fmt,
  diff check и public-tree проходят. Новый независимый review и аппаратная
  проверка D остаются открытыми. E–G этим исправлением не изменялись.

## 6. Объединить состояние кадра и сохранить входные данные — R4, R5

**Файлы:** `gfx.rs`, `gfx/{dlss,motion,world_post}.rs`,
`horizon-streamline/src/{runtime,reflex,super_resolution,frame_gen,evaluate}.rs`.

### 6.1. Один token и один набор common constants

- [x] Определить владельца состояния одного показываемого кадра и явные этапы
  begin, подготовка входов, submit, present, завершение либо отмена.
- [x] Использовать один frame token для Reflex, SR и FG одного настоящего кадра.
- [x] Строить common constants один раз после определения размеров и reset;
  после передачи в SDK не изменять их в рамках этого кадра.
- [x] Убрать самостоятельную повторную сборку constants внутри FG.
- [x] Хранить валидность запасной истории TAA отдельно от reset текущего кадра.
- [x] Определить reset для первого кадра, camera cut/teleport, смены мира,
  размера, режима и возврата после приостановки.
- [x] Проверить ранние выходы в коде: не полученная surface texture, пропуск рендера,
  SDK error, surface loss и завершение приложения.
- [x] Ошибка передачи constants исключает генерацию этого кадра; повторная
  попытка следует явной политике восстановления без бесконечного спама логов.
- [x] Проверить расстановку маркеров Reflex в коде относительно ввода, подготовки
  кадра, всех относящихся к нему submit и Present, включая отдельный server thread.

### Повторное исправление F1 (1 ноября 2026, review pending)

- Поздний `slUpgradeInterface(swapchain)` заменён instance-scoped HAL
  интеграцией: factory proxy существует до создания surface, выбранный native
  device регистрируется и получает proxy до CreateCommandQueue. Все поддержанные
  CreateSwapChainForHwnd/Composition, GetBuffer, ResizeBuffers и Present проходят
  через полученные proxy. IDXGIFactoryMedia surface handles явно отклоняются
  при Streamline, поскольку SDK не предоставляет для них creation hook.
- HAL сохраняет создающий proxy device: SDK command queue заимствует его
  без AddRef. COM cast запрашивает точную версию интерфейса и освобождает
  переданную SDK owned reference. При teardown SDK lease удерживает native
  и proxy device до slShutdown, затем освобождает COM, DLL и global lease;
  HAL также удерживает d3d12.dll. Цикла Runtime → device → Runtime нет.
  Порядок shutdown Vulkan, Off/wait/reset/error guards и NVIDIA Off сохранены.
- Новый явный `official_sdk_dx12_creation_buffers_resize_and_recreate`
  прошёл на RTX 5090 / 621.01 / SDK 2.14.16.0: factory, queue и chain
  подтверждены через slGetNativeInterface; acquired textures HAL равны
  proxy GetBuffer и отличаются от native GetBuffer. Проверены два новых chain,
  каждый 800→960→800, clear/submit/Present и полное освобождение SDK lease
  после раннего drop Runtime. Это настоящий creation/Present тест с FG Off,
  а не проверка focused FG 2× или дополнительного показанного кадра.
- Windows: Streamline ordinary 21 passed / 7 ignored; explicit official SDK
  4 passed; real SR pixel tests Vulkan/DX12 2 passed;
  renderer surface_lifecycle 4 passed; targeted check и Clippy
  renderer/Streamline lib/tests с `-D warnings` прошли. Full pr_ready,
  Linux/macOS и активная оконная FG не выполнялись для F1.
- MIT license и заголовки закреплённого SDK проверены до чтения источников;
  перечень прочитанного добавлен в THIRD_PARTY_NOTICES.md. SDK, подготовленные
  ресурсы и локальные логи не включаются в commit. F2–F5 этим этапом не закрыты.

### Результат кода E (31 октября 2026)

- База E: `45e631a73e533be69d0fbb96d5b2fee6b7d71117`. Независимый
  `review_d2` дал **PASS коду D** на этой базе; аппаратная проверка D открыта.
  Public-tree для D повторно проверен основным агентом с точным путём Python.
- Runtime владеет одним rendered-frame token и immutable common constants.
  Renderer открывает кадр после Reflex begin и замораживает камеру/jitter/reset
  после motion, до SR. FG получает тот же snapshot и больше не вызывает
  slSetConstants самостоятельно. При Reflex Off token выделяется один раз
  лениво; первый потребитель отправляет constants, следующие только сверяют их.
  Сверка по полям, без padding; non-finite floats отклоняются до SDK.
- Render wrapper закрывает кадр на всех возвратах draw, включая acquire
  timeout/occlusion/loss, ошибки encode и пропуск из-за pending resize.
  Отменённый кадр не имеет Present, сбрасывает историю следующего eligible
  кадра и не вызывает дополнительный Off, обходящий backoff части C.
  Смена мира через clear_sections и смена AA/scale сбрасывают temporal history;
  camera cut/teleport берёт reset из motion. Pending C/D и readiness hook
  сохраняются. Изменения сохранения depth и tagging flag оставлены F/G.
- Успешный SR инвалидирует только запасные TAA textures. Current-frame jitter
  остаётся неизменным. При возврате fallback его первый resolve получает reset,
  следующий уже использует историю. Ошибка SR запрещает FG этого кадра.
  Ошибка constants/tag/options сначала проверенно выключает FG; ошибка Off
  возвращается из render без Present. Constants можно повторить в следующем
  кадре с reset; три последовательных неудачных кадра отключают FG до restart.
  Sticky SR fallback по ошибке сохраняется до смены выбора пользователем.
- Reflex SubmitStart перенесён перед первым world/SR submit, SubmitEnd остаётся
  после финального submit. Повторный Start подавляется; abandon закрывает
  открытые simulation/submit markers и token. Server-thread claim остаётся
  привязан к тому же открытому Reflex token.
- Production RenderFrame regression: пять кадров SR+FG, один SDK constants
  call и один token на кадр, reset `true,false,false,true,false`; ошибка SDK,
  изменение constants, non-finite input и закрытый кадр блокируют потребителя.
  Настоящий wgpu Post regression: четыре SR кадра дают shared reset `1,0,0,0`,
  инвалидирование fallback его не меняет; TAA fallback reset `1,0,0`, затем
  проверены abandon/reset и смена AA.
- Windows: Streamline **14 passed, 3 ignored**; world_post **5 passed**,
  lifecycle C/D **6 passed**, surface recovery **4 passed**, dlss **3 passed**,
  motion **2 passed**. Clippy Streamline/renderer/client lib/bins/tests с
  `-D warnings`, fmt, diff check и public-tree проходят.
- Официальный SDK 2.14.16: **3 ignored tests passed serially** (requirements,
  настоящий SR DX12 и Vulkan с проверкой пикселей). SR tests дополнены shared
  begin/common-constants/end и повторными запросами того же token после SR.
  Sandbox SDK requirements завис на CPU; тестовый процесс остановлен, повтор
  вне sandbox успешно завершён. SDK отклонил подпись NvLowLatencyVk.dll,
  поэтому этот прогон не доказывает Vulkan Reflex.
- Независимый `review_e` дал **PASS** для кода E на
  `f2b07d368d5c9f1d94a01c3c2204ae0315e94785` и повторил 3 SDK SR tests.
  Настоящий FG Present, визуальное
  качество, latency, stress focus/loss/resize и Linux/macOS не проверялись.
  Флаг frame-based tagging пока не включён; аппаратную готовность FG этот
  результат не заявляет. Полный pr_ready и parity не запускались.

### 6.2. Depth и motion должны доживать до потребителя

- [x] Вычислять необходимость сохранения depth по всем активным потребителям:
  TAA, SR, FG, motion, OIT, shader packs и culling.
- [x] Проверить StoreOp в последнем проходе, записывающем depth, включая entity
  и translucency paths; промежуточного Store недостаточно.
- [ ] Проверить соответствие color/depth/motion по кадру, размеру, проекции,
  jitter, направлению движения, единицам измерения и reverse depth.
- [ ] Проверить порядок barriers, копирования, native command lists и Present
  на Vulkan и DX12, а также срок жизни texture/view и raw handles.
- [x] При неполных или устаревших входах запрещать FG текущего кадра.
- [ ] Добавить GPU-проверку содержимого depth/motion при FG On, AA Off/FXAA,
  OIT Off и culling Off; отдельно проверить включённые TAA/OIT/culling.
- [ ] Добавить тест SR+FG: constants согласованы, ложного reset на каждом кадре
  и `eErrorDuplicatedConstants` нет; fallback TAA начинает с валидной истории.

**Готово, когда:** у кадра один согласованный набор данных; SDK не видит
разных constants для одного token, а FG не читает discarded/stale depth.

### Результат кода F и GPU-проверок (31 октября 2026)

- Сохранение depth учитывает TAA, SR/FG, motion diagnostics, packs и OIT.
  Первый world pass сохраняет его для culling, последний — для последующих
  потребителей. Flat/indirect culling, entity/translucency и проход после OIT
  используют общий production world-pass constructor. OIT depth bounds/cull,
  composite и water-mask сохраняют depth; read-only passes его не отбрасывают.
- Motion diagnostics без AA тоже получают отдельный world target и историю
  камеры. Рука, crosshair и HUD используют screen depth, не depth мира.
  Согласованы Depth32Float/Rg16Float и размер входов; motion отвергает неверный
  размер, singular/non-finite projection, camera/jitter. Reset скрывает старый
  motion target. FG допускается только после успешной записи motion именно
  в этом render; пропущенный pass не переиспользует прошлый motion image.
- `gfx::temporal_input_tests` — **2 ignored GPU tests passed** на RTX 5090,
  драйвер 621.01 (DX12 32.0.16.2101), Vulkan и DX12. На каждом API 12 вариантов:
  AA Off/FXAA/TAA × culling-final-pass Off/On × OIT-final-attachment Off/On,
  два кадра каждый. Рисуются треугольники с depth 0.25/0.75; GPU readback
  проверяет обе глубины, reset, ненулевой depth-dependent motion в UV units
  (previous minus current, Y вниз) и jitter. Проверка идёт после post/hand clear.
  Используются настоящие Post/Motion, production StoreOp и tagged textures.
- Negative mutation восстановила прежнее условие без FG/motion: Vulkan test
  упал на AA Off/culling Off/OIT Off, depth 0.0 вместо 0.25. Исходник точно
  восстановлен, оба API повторно прошли. Отдельно проверен отказ при неверном
  размере и singular projection без выдачи старого motion target.
- Motion **2 passed**, world_post **5 passed**, DLSS **3 passed**; Clippy
  renderer/client lib/bins/tests с `-D warnings`, fmt, diff/public-tree проходят.
  GPU tests запускаются явно: `cargo test -p horizon-client-render --lib
  temporal_input_tests -- --ignored --nocapture --test-threads=1`.
- Это headless GPU harness, не полный Renderer/game и не native FG Present.
  Culling проверяет production final attachment, без visibility dispatch;
  OIT — final attachment, без полного translucent pipeline. Native barriers,
  queue order и lifetime рассмотрены по коду: world/SR submit → final draws /
  RESOURCE transitions → отдельный tag list → Present; owned textures остаются
  у Post/Motion. Настоящий SDK copy/Present, stress, визуальное качество motion,
  Linux/macOS остаются открытыми. Независимый review_f дал PASS коду F на
  `e5a51229a5ec6c5f7682bdb6da29c968356fe34e`. На этапе F флаг G не включался;
  полный pr_ready/parity не запускались. SDK и игровые файлы не добавлялись.

## 7. Исправить контракт Streamline и доказать работу FG — R2

Зависит от этапов **5 и 6**. Исправление флага отдельно не считается готовой FG:
оно сделает достижимыми ранее скрытые ошибки жизненного цикла и ресурсов.

- [x] Использовать именованные константы закреплённого SDK для Preferences.flags.
- [x] Включить `eUseFrameBasedResourceTagging` для выбранного пути тегирования.
- [x] Проверить совместимость нового режима с локальными tags SR и очисткой
  frame-based tags при приостановке FG.
- [x] Проверить поддержку FG/Reflex на фактическом устройстве и доступность
  нужных модулей; наличие DLL и найденных функций не считать доказательством.
- [x] Разобрать сообщение SDK о неподписанной `NvLowLatencyVk.dll`: установить
  допустимый официальный комплект и подтвердить Vulkan Reflex; проверки
  подписи не обходить и не отключать.
- [ ] Сначала подтвердить настоящий режим 2× в оконном приложении с Present.
- [ ] Проверить успех tagging, constants и options, отсутствие SDK warnings/errors
  и фактические `numFramesActuallyPresented`.
- [x] Считать показанные FPS по реально прошедшему времени, отдельно от настоящих
  кадров; не подменять измерение выбранным в меню множителем.
- [ ] Разрешать 3×/4×/6× только в пределах подтверждённых возможностей устройства;
  показывать эффективный режим при ограничении запроса.
- [ ] Проверить FG Off/On, Reflex On/Boost и ошибку SDK с переходом к настоящим
  кадрам; отключение FG не обязано отключать Reflex.
- [ ] Зафиксировать ограничение HUD/руки в сгенерированном изображении. Вынос
  HUD/руки в отдельный путь оформить следующей самостоятельной задачей, если
  он необходим для принятого уровня качества.

**Готово, когда:** на Vulkan и DX12 доказан показ дополнительных кадров,
отсутствуют ошибки интеграции и симуляция не ускоряется от выбранного множителя.

### Результат кода G и границы проверки (1 ноября 2026)

- Preferences используют именованные флаги публичного MIT SDK 2.14.16:
  Vulkan `0x81`, DX12 `0x85`; без OTA и обходов проверок. Production startup
  проверяет `slIsFeatureSupported` для FG/Reflex/PCL на зарегистрированном
  адаптере и `ReflexState.lowLatencyAvailable`. Отказ отключает потребителя,
  сохраняя обычный рендер. Повторный уже подтверждённый Off не отправляется;
  ошибка Off и последующий On сохраняют необходимость корректного suspend.
- Официальный `bin/x64/NvLowLatencyVk.dll` отсутствовал в локальном native
  комплекте: сообщение «NOT correctly signed» само по себе не доказывало
  неподписанный существующий файл. DLL из официального SDK zip установлена
  только в `.local/`; SHA256
  `2A77DC3E1C724B7EEA5755BE0AE7423752E79A2459FAE72181A9F00E3507E5D6`.
  Windows Authenticode — Valid/NVIDIA; сам Streamline подтвердил подпись.
  Проверки подписи не отключались, сторонние бинарники не использовались.
- RTX 5090, драйвер 621.01: пять ignored SDK tests прошли последовательно.
  Два новых теста подтверждают FG/Reflex/PCL на Vulkan/DX12, фактическую
  доступность low latency и успех Reflex On/Boost/Off. Оба SR pixel tests
  отправляют настоящие null frame-tags до/после local SR tags на одном token;
  SR readback и общий constants/token остаются валидными.
- Показанные FPS считаются из SDK `numFramesActuallyPresented / elapsed`,
  настоящие — из числа application presents за тот же интервал. Множитель
  не участвует в измерении. Unit test включает задержку 2–4 секунды и SDK
  counts 0/1/2. Control status экспортирует строки FG/Reflex для диагностики.
- Ordinary Streamline: **17 passed, 5 ignored**; renderer `gfx::tests` —
  **23 passed**. Clippy Streamline/render/client lib/bins/tests с `-D warnings`,
  fmt, diff check и public-tree прошли. Полный pr_ready/parity не запускался.
  Локальный SDK test log: `.local/dlss/logs/g-sdk-tests.txt`.
- Оконный DX12 game smoke с отдельными scratch homes достиг настоящего
  Present, загрузил FG (SDK max generated 5) и Reflex (измерение 9.4 ms),
  но FG оставалась paused без фокуса. Он выполнялся после исправления flags,
  до последующих G support/rate/Off изменений. SDK сообщил repeated initial
  Off (добавлена регрессия/дедупликация) и advisory о синхронизации GetState;
  все эти calls/Present имели один tid. Advisory не объявляется устранённым.
- Навык [computer-use](C:/Users/godsp/.codex/plugins/cache/openai-bundled/computer-use/26.1027.30824/skills/computer-use/SKILL.md)
  применён для фокуса: CUA Raise вернул `GetCursorPos: access denied
  (0x80070005)`, восстановление observation было aborted before dispatch;
  UI-ввод остановлен. Известный тестовый PID завершён через control quit.
  **Настоящая FG 2× на Vulkan/DX12, live generation tags, warnings-free
  Present, качество HUD/руки и stress остаются непроверенными.** Headless
  SDK support/SR tests их не заменяют.

### Исправление по review G (1 ноября 2026)

- Независимый `review_g` дал **FAIL** для
  `5c3e9ded2cf72a35d7532f12bb8dc64abaca7e75`: новая общая проверка адаптера
  ошибочно применяла DLSS RTX-name prefilter к Reflex/PCL. Это отклоняло
  поддерживаемые SDK устройства GeForce GTX до `slIsFeatureSupported`.
- Production boundary теперь сохраняет общую проверку API/session identity,
  а RTX prefilter применяет только к DLSS/FG (0/1000). Reflex/PCL (3/4)
  получают ответ SDK на фактическом native adapter, независимо от имени GPU.
  Это следует закреплённому официальному Reflex guide: Reflex Low Latency
  поддерживает GeForce 900+; PCL работает и на других производителях.
- Регрессия вызывает тот же production boundary на Vulkan/DX12: GTX 1080
  достигает mock SDK для Reflex/PCL, успех и отказ SDK сохраняются; DLSS/FG
  по-прежнему отсеиваются; неверный API не достигает SDK; Radeon PCL достигает.
  **18 ordinary tests passed, 5 SDK tests ignored** в этом повторном прогоне.
  Clippy Streamline/render/client lib/bins/tests с `-D warnings`, fmt,
  diff check и public-tree прошли.
  Проверка GTX на настоящем оборудовании не выполнялась. Независимый
  `review_g2` дал **PASS** коду `8d100b1dd0ef9f81f2416a40a151831fd95bef93`;
  прежние ограничения actual FG 2× остаются открытыми.

### Результат кода H и границы проверки (1 ноября 2026)

- `numFramesToGenerateMax = 0` означает отсутствие подтверждённого лимита:
  FG ждёт новый успешный GetState, прежний высокий cap не сохраняется.
  3×/4×/6× появляются при переключении меню только в пределах cap SDK;
  2× остаётся выбираемым запросом для загрузки при restart. Native On тоже
  ждёт ненулевой cap. Запрос из сохранённых настроек ограничивается cap,
  runtime показывает **configured/requested**, отдельно SDK shown/rendered FPS.
  Нулевые SDK presents остаются нулевыми, настройка не доказывает генерацию.
- Неуспех GetState, ненулевой `DLSSGState.status`, SetOptions или live tags
  переводит FG в fallback: checked Off, reset, очистка tags того же token.
  Неуспешный live tag list не отправляется. Следующий On требует явной смены
  режима; повторная та же ошибка не логируется каждый кадр. Минимальный размер
  SDK тоже ограничивает eligibility. Отказ Off по-прежнему передаётся вызывающему
  коду; неподтверждённое выключение не объявляется безопасным.
- FG требует успешно включённый Reflex с подтверждённой доступностью.
  Неуспешные Reflex options пробуются один раз на режим; смена режима разрешает
  повтор. Статус различает Off, ошибку, отсутствие latency reports и ожидание
  отчётов; старое latency после ошибки/отсутствия отчётов не показывается.
  Отключение FG сохраняет Reflex; правило Off → Reflex On сохранено.
- Ordinary Streamline: **20 passed, 6 ignored**. Новый ignored production record
  test запущен явно и прошёл (**1 passed**); он
  использует настоящие Vulkan/DX12 encoders и mock SDK: отказы tags/options,
  только cleared tags в submit, checked Off, retry, cap 1 при запросе 5,
  нулевые presents и minimum size. Отдельные mock tests проверяют status/code
  GetState, сброс cap, дедупликацию ошибок и Reflex options/availability.
  Renderer graphics/UI — **25 passed**, `gfx::tests` — **23 passed**.
  Clippy Streamline/render/client lib/bins/tests с `-D warnings`, fmt,
  diff check и public-tree прошли. Полный pr_ready/parity не запускался.
- Пять ignored tests официального SDK повторно прошли последовательно с новыми
  scratch homes (**5 passed**): support/Reflex на обоих API, requirements и два SR
  pixel tests. Локальный лог: `.local/dlss/logs/h-sdk-tests.txt`.
- Это обработка ошибок и статусов, а не доказательство реальной генерации.
  Фокусированный оконный FG Present на Vulkan/DX12, HUD/рука, stress и advisory
  синхронизации GetState остаются открытыми после недоступности CUA в G.
  Обходы ограничений доступа или проверки подписей не применялись.
  Нужен независимый review H.

### Исправление по review H (1 ноября 2026)

- Независимый `review_h` дал **FAIL** коду
  `dd011cbd9aaa6462e245d329b6ffc5f6070ef53c`: latched SDK fault сохранял
  `loaded` и pending reset, поэтому каждый eligible fallback-кадр сбрасывал
  общую Post/Motion историю. То же происходило при cap 0 и недоступном Reflex.
- Runtime теперь возвращает применимый pending reset только при загруженной,
  запрошенной FG без fault, подтверждённом ненулевом cap, активном Reflex и
  готовом presentation hook. Запрос остаётся в Session без изменения до
  настоящей попытки; recovery сбрасывает историю до построения constants.
- **21 ordinary Streamline tests passed, 6 ignored**. Новый Session test
  проверяет каждый blocker, сохранение pending и разрешение первого recovery.
  Новый ignored real Post/Motion test запущен явно: **1 passed**, Vulkan/DX12,
  TAA fallback и SR, последовательность reset `1,0,0,0,1,0`, накопление TAA,
  согласованный Motion reset и неизменные cached common constants. Это две
  границы проверки: Session readiness и обработка её результата renderer.
  Clippy Streamline/render/client lib/bins/tests с `-D warnings`, fmt,
  diff/public-tree прошли. Новые SDK/game запуски не требовались; ограничения
  H по настоящему FG Present остаются открытыми. Нужен новый review H.

### Исправление по review H2 (1 ноября 2026)

- `review_h2` дал **FAIL** коду `112a84fdb18a2472be7b8b2697d289587b7cc5cc`:
  ранняя readiness не учитывала `minWidthOrHeight`, хотя поздний record
  приостанавливал FG в маленьком окне. Это снова повторяло reset fallback.
- Renderer передаёт настоящий размер swapchain в Runtime readiness. Один
  `dimensions_supported` проверяет обе стороны против SDK minimum и в раннем
  reset gate, и в record. Pending сохраняется до допустимого размера.
- Session test проверяет отдельно малую ширину/высоту, обе малые стороны и
  точную границу. Real Post/Motion regression дополнен настоящими 16×16
  targets/output с восстановлением до 32×32, для TAA/SR на Vulkan/DX12:
  reset `1,0,0,0,1,0`. **21 ordinary passed, 6 ignored**, явный GPU test
  **1 passed**; Clippy затронутых Streamline/render/client lib/bins/tests,
  fmt, diff/public-tree прошли. SDK/game повторно не запускались.
- Остальные ранние gates проверены: mode/load/fault/cap/Reflex/presentation и
  контекст окна. Depth/motion/constants строятся позднее; отсутствие token
  отмечается Reflex до ранней проверки. Checked surface suspend очищает старый
  размер; ошибки поздних входов сохраняют reset/fault для следующего кадра.
  Аппаратные ограничения H остаются открытыми. Независимый `review_h3` дал
  **PASS** коду `dbde495af566192b488df99937aa6956248e22d3` после этих исправлений.

## 8. Выполнить итоговую проверку качества

### 8.1. Матрица сценариев

Для каждой строки сохранить SHA, API, настройки, результат, лог и при ошибке
способ воспроизведения. Аппаратные проверки выполнить на Vulkan и DX12.
Все чекбоксы этой матрицы означают полную аппаратную приёмку сценария.
Unit/headless GPU/SDK-подтесты перечислены отдельно в
[итоговом отчёте](dlss-validation.md) и не закрывают эти строки целиком.

| Сценарий | Ожидаемый результат | Проверено |
| --- | --- | --- |
| Все NVIDIA-функции Off | Обычный рендер, без загрузки SDK | [ ] |
| SR без FG, все качества и DLAA | Правильный размер, стабильное изображение | [ ] |
| Reflex без SR/FG | Корректные маркеры и измерения, штатный FPS cap | [ ] |
| FG без SR, AA Off/FXAA | Валидные depth/motion, дополнительные кадры | [ ] |
| SR + Reflex + FG | Общие token/constants, без постоянного reset | [ ] |
| OIT/culling Off и On | Оба варианта сохраняют нужные входы | [ ] |
| Resize/fullscreen/minimize/restore | Без deadlock и чтения старых ресурсов | [ ] |
| Новая surface после потери старой | Hook восстановлен, функции возобновляются | [ ] |
| Title/menu/pause/focus loss/HDR/shader pack | Документированный режим приостановки | [ ] |
| SDK отсутствует/не поддержан/возвращает ошибку | Настоящий кадр и точная причина fallback | [ ] |
| Смена мира/телепорт/переключение качества | Корректный reset без длительных следов | [ ] |
| Длительный прогон с переключениями | Без роста ресурсов и зависаний | [ ] |

### 8.2. Автоматические проверки и разбор старых ошибок

- [ ] Запустить fmt, Clippy, unit/integration/doc tests затронутых пакетов.
- [x] Повторить обычные и аппаратные тесты `horizon-streamline`, motion и настроек.
- [x] Выполнить `python tools/pr_ready.py --base 763528025 --quick` и сохранить
  полный результат; для будущего PR повторить относительно его реальной базы.
- [x] Разобрать 17 failures/4 errors Python и 27 failures launcher из review:
  воспроизвести на сопоставимой базе и отделить среду от регрессий ветки.
- [ ] Разобрать зависания двух `server_packs::download` тестов; ограничить ожидание
  при повторном диагностическом запуске и сохранить вывод зависших тестов.
- [ ] Выполнить выбранный parity-набор там, где подготовлены vanilla-оракулы;
  отсутствие ресурсов отметить явно. Не запускать устаревший in-frame gameplay.
- [x] Проверить публичное дерево и историю, `git diff --check`, состав Cargo.lock.
- [ ] Провести визуальную проверку владельцем: повороты камеры, листья, края
  блоков, мобы, предметы, рука и HUD в лёгкой и тяжёлой сцене.
- [ ] Сопоставить frametime, настоящие/показанные FPS, latency и плавность
  в одинаковых сценах и настройках. Счётчик FPS сам по себе недостаточен.

**Готово, когда:** обязательные проверки завершены; у каждого оставшегося сбоя
есть установленная причина и доказательство, что это не регрессия переделки.
Непроведённые аппаратные/платформенные проверки остаются открытыми чекбоксами.

### Итог локальной проверки I (1 ноября 2026)

Self-check на коде `dbde495af` завершён с **FAIL**, а не общей отметкой готовности:
500 Python tests, 2 failures/2 skipped; эти два падения воспроизведены на архивной
базе `763528025` с сопоставимыми `.local` temp paths и неизменёнными parity/test
исходниками. Остальные прежние sandbox/MSYS/launcher сбои не повторились вне
sandbox: launcher 297 passed/4 ignored. Fmt/Clippy, doc и 6 content-root tests
прошли. Workspace принудительно остановлен по лимиту 180 секунд в параллельном
renderer-прогоне; 1469 завершённых passes не считаются полным результатом.
Один отдельный renderer-прогон с одним test thread прошёл: 719 passed/24 ignored
за 33,53 с. Причина parallel stall не установлена, этот пункт остаётся открытым.
Оба прежних зависавших server_packs download tests теперь прошли в полном
client lib (924 passed/22 ignored).

Повторены Streamline ordinary (21 passed/6 ignored), official SDK support/SR
(5 passed), error-path GPU mock (1), temporal inputs GPU (2), blocked history
GPU (1), motion (5), graphics/UI (25). Все прошли последовательно с fresh homes.
Public tree/range `763528025..HEAD`, diff check и проверка состава lockfile прошли.
pr_ready перечислил 359 harnesses, прямой dependency list — 367; oracle runs не
выполнялись. Подготовленные renderer resources установлены в launcher и доступны
только для чтения; `.local/minecraft` с Java-оракулами отсутствует. Полные локальные
логи — `.local/I/`; переносимые команды, baseline-разбор и границы доказательств —
[dlss-validation.md](dlss-validation.md). Аппаратная матрица и Linux/macOS открыты.

## 9. Обновить документацию и провести повторный review — R9

- [x] Обновить `docs/dlss-plan.md`: фактический статус SR, Reflex, FG и оставшихся
  этапов вместо противоречащих реализации утверждений.
- [x] Обновить `docs/graphics.md`: API, требования, restart, fallback, ограничения,
  эффективный множитель и поведение при смене состояния окна.
- [x] Привести ручную инструкцию к текущим названиям меню и изолированному запуску.
- [x] Добавить воспроизводимые команды проверки без персональных путей.
- [x] Отделить «реализовано», «прошло автоматические тесты» и «визуально проверено».
- [ ] Провести повторный review итогового diff и каждого исправления R1–R9.
- [ ] Подтвердить отсутствие новых нарушений AGENTS.md/CLAUDE.md/CONTRIBUTING.md.
- [ ] Проверить, что все критерии готовности этапов закрыты доказательствами.

## 10. Порядок выполнения и разбиение изменений

Последовательность: **3 → 4 → 5 → 6 → 7 → 8 → 9**.
Подготовка документации может идти по мере реализации; финальные результаты
проверок вносятся после этапа 8.

Каждое изменение должно быть самостоятельно проверяемым; ориентир репозитория —
до примерно 500 строк на PR. Большой этап разбивать на несколько связных частей,
а тесты включать вместе с исправлением соответствующего поведения.

- [x] Часть A: изолированный запуск и атрибуция ABI.
- [ ] Часть B: код платформенной зависимости и заглушек реализован; Windows
  проверен, подтверждение Linux/macOS-сборок ещё требуется.
- [ ] Часть C: код отключения FG до изменения swapchain и regression tests
  прошёл review_c4 PASS; аппаратная проверка открыта.
- [ ] Часть D: код восстановления DX12 proxy при замене surface реализован;
  targeted tests прошли, review_d2 дал PASS коду; аппаратная проверка открыта.
- [ ] Часть E: общий frame token/constants и отдельная валидность TAA history
  прошли review_e PASS и targeted/SDK SR tests; аппаратная проверка открыта.
- [ ] Часть F: сохранение depth/motion и headless GPU readback на Vulkan/DX12
  реализованы и проверены; review_f PASS, native FG/game проверка открыта.
- [ ] Часть G: код frame-based tagging, support и FPS реализован, SDK-контракты
  и review_g2 прошли; настоящая FG 2× остаётся открытой.
- [ ] Часть H: обработка ошибок, подтверждённые множители и статусы реализованы;
  targeted/GPU tests и review_h3 прошли; аппаратная проверка открыта.
- [ ] Часть I: финальная документация и отчёт проверки реализованы;
  независимый review требуется, общая аппаратная приёмка открыта.

### Финальный критерий приёмки

- [ ] R1–R9 исправлены и повторно проверены.
- [ ] Windows/Linux/macOS собираются; обычный путь с NVIDIA Off работает.
- [ ] SR, Reflex и FG проверены отдельно и совместно на обоих заявленных API.
- [ ] Resize/surface loss/error paths проходят без deadlock и перезапуска игры.
- [ ] Общий self-check имеет достоверный результат; оставшиеся ограничения явно
  описаны и не скрыты отметкой «всё прошло».
- [ ] Пользователь подтвердил приемлемое визуальное качество и поведение ввода.
