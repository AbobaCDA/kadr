# Быстрый старт на Windows

В архиве — **исходный код**, а не готовый `exe`. Так устроены все Tauri-приложения:
бинарник собирается один раз на твоей машине, дальше запускается как обычная программа.
Первая сборка — 10–20 минут, потом пересборка занимает секунды.

## Вариант 0 — вообще ничего не устанавливать (сборка в облаке)

GitHub соберёт exe за тебя, на своей виндовой машине. Твой компьютер нужен только чтобы
залить код — 30 МБ интернета, без Rust, без Visual Studio, без Node.

```powershell
cd C:\путь\до\kadr
powershell -ExecutionPolicy Bypass -File tools\publish.ps1
```

Скрипт сам: поставит коммит, создаст репозиторий `AbobaCDA/kadr` и запушит его (если
установлен [GitHub CLI](https://cli.github.com) — полностью автоматически, иначе даст одну
команду `git push`), выдаст права Actions, а в конце напечатает ссылку.

Дальше:

1. Открой напечатанную ссылку → **Run workflow** → **Run workflow** (зелёная кнопка справа).
2. Через ~20 минут внутри запуска появится артефакт `kadr-windows-…` — это zip
   с установщиком `kadr_1.0.16_x64-setup.exe` и портативным `kadr-portable.exe`.
3. Скачал, распаковал, запустил. Всё.

Любой `git push` в `master` пересобирает exe автоматически. Хочешь постоянную ссылку на
файл — поставь тег:

```powershell
git tag v1.0.16
git push --tags
```

Тег создаёт **Release** с прикреплённым exe (`Releases` в правой колонке репозитория).

Нужен `git`? `winget install --id Git.Git -e`, либо просто GitHub Desktop.
Флаг `-Private` в `publish.ps1` создаст приватный репозиторий (Actions там тоже работают).

## Вариант 1 — одна команда (ставит всё сама, сборка локальная)

Открой PowerShell **от имени администратора** (нужно для Visual Studio Build Tools),
перейди в папку с исходниками и запусти:

```powershell
cd C:\путь\до\kadr
powershell -ExecutionPolicy Bypass -File tools\setup-windows.ps1
```

Скрипт проверит и при необходимости поставит через `winget`:

- Rust (rustup, MSVC-тулчейн);
- Visual Studio Build Tools с workload «Desktop development with C++»;
- Node.js LTS;
- проверит WebView2 (на Windows 10 2021+ и 11 он уже есть).

Затем сделает `npm ci` → `npm run dist` и покажет путь к установщику.

Полезные флаги:

```powershell
# запустить приложение сразу, без сборки установщика (hot reload)
powershell -ExecutionPolicy Bypass -File tools\setup-windows.ps1 -Dev

# компактная сборка: ~7 МБ вместо ~9 МБ (без PP-OCR/ONNX)
powershell -ExecutionPolicy Bypass -File tools\setup-windows.ps1 -Slim

# всё уже установлено — только собрать
powershell -ExecutionPolicy Bypass -File tools\setup-windows.ps1 -SkipPrereqs
```

## Вариант 2 — вручную, локально

Если не хочешь запускать скрипт, или `winget` недоступен:

1. **Rust** — <https://rustup.rs> (по умолчанию ставится `stable-x86_64-pc-windows-msvc`).
2. **Visual Studio Build Tools 2022** — <https://visualstudio.microsoft.com/visual-cpp-build-tools/>,
   отметить **Desktop development with C++**. Это даёт `link.exe`, без него Rust не соберётся.
3. **Node.js 20+** — <https://nodejs.org> (LTS).

Дальше в обычном PowerShell:

```powershell
cd C:\путь\до\kadr
npm ci
npm run dist
```

Установщик появится здесь:

```
src-tauri\target\release\bundle\nsis\kadr_1.0.16_x64-setup.exe
```

Портативная версия (без установки) — `src-tauri\target\release\kadr.exe`.

## Что будет после сборки

- Запусти установщик (или `kadr.exe`) → мастер первого запуска попросит выбрать папки
  для индексации и начнёт первый проход.
- Поиск: просто пиши. `image`, `фото`, `screenshot` превращаются в фильтр
  `type:image` и листаются до конца по всем найденным файлам.
- Скил: выдели картинку или видео в результатах (стрелки) и нажми **Ctrl+D**.
  Первый раз спросит папку — дальше будет молча переносить туда файлы.
  Переключить на «копировать», сменить папку или сочетание:
  **Настройки → Общие → Коллекция**.

## Портативный режим

Положи рядом с `kadr.exe` пустой файл с именем `portable` — конфиг, индекс,
метаданные и миниатюры будут лежать в `<папка exe>\data`. Ничего не пишется в
`%APPDATA%`, ничего не нужно устанавливать. Удобно держать на внешнем диске.

## Если что-то не так

| Симптом | Что делать |
|---|---|
| `linker 'link.exe' not found` | Build Tools без workload C++. Открой Visual Studio Installer → Modify → добавь «Desktop development with C++». |
| `'cargo' is not recognized` | Перезапусти PowerShell (PATH обновится только в новом окне), затем `-SkipPrereqs`. |
| Приложение не стартует, окно пустое | Нет WebView2: поставь Evergreen Runtime. |
| Антивирус ругается на установщик | Ожидаемо для неподписанного бинарника. Подпиши через `signtool` или добавь исключение. |
| Хочется меньше размер | `npm run dist:slim` (~7 МБ) или `npm run size:slim` + обычная сборка. |
| Хочу и в облаке, и slim | Actions → Run workflow → галочка **slim**. |
| Release-шаг упал с 403 | Settings → Actions → General → Workflow permissions → Read and write. |
| Другое имя/иконка | `tools/rebrand.sh "Имя" "app.твой.id" "exe-name"`, затем `npm run dist`. |

Подробности по всем рычагам размера и троублшутингу — [BUILD.md](BUILD.md).

## Логи

`%APPDATA%\kensaku\kensaku.log` — там же, где конфиг и индекс. Если что-то падает,
пришли этот файл.
