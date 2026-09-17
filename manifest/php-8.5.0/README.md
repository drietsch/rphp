# PHP 8.5.0 oracle manifest

Reflection dump of the stock PHP this project is measured against:

```
PHP 8.5.0 (cli) (built: Nov 18 2025 08:02:20) (NTS)
```

(Homebrew `php` on macOS arm64, `/opt/homebrew/bin/php`.) Regenerate with,
from the repository root:

```
php -n tools/manifest/dump.php manifest/php-8.5.0
```

`-n` skips every php.ini, so:

- `ini.json` holds the **compiled-in** defaults (`global_value`/`local_value`
  are what `ini_get_all(null, true)` reports with no ini file loaded), and the
  extension set is the one statically linked into this build (65 extensions;
  nothing loaded via `extension=` lines).
- Runtime-computed constants in `constants.json` (`PHP_BINARY`, `PHP_OS`,
  `PHP_OS_FAMILY`, `DIRECTORY_SEPARATOR`, `STDIN`/`STDOUT`/`STDERR`, the
  `PHP_*DIR` install paths, …) are environment specific; `cargo xtask gen`
  emits them as `ConstValue::Runtime` and the engine computes them at startup.

| File | Keyed by | Contents |
|---|---|---|
| `functions.json` | function name | every `get_defined_functions()['internal']` entry: extension, deprecated, returns_ref, return type, params (type, nullable, by_ref, prefer_ref, variadic, optional, default + raw `default_expr` stub text) |
| `classes.json` | class name | every internal class/interface/trait/enum: kind, abstract/final/readonly, parent, direct + all interfaces, own constants/properties/methods (declaration order), enum backing type + cases, attributes |
| `constants.json` | category → constant name | `get_defined_constants(true)`: value (`{"expr": "NAN"}`, `{"resource": "stream"}` for the unrepresentable ones), PHP type, deprecated |
| `ini.json` | directive name | `ini_get_all(null, true)` plus the owning extension |
| `extensions.json` | extension name | `phpversion($ext)`, dependencies, function/class/constant/ini counts |

All maps are key-sorted and the JSON is pretty-printed so diffs between PHP
versions stay readable. Parameter defaults: a plain JSON value, `{"const":
"PHP_INT_MAX"}` when `ReflectionParameter::isDefaultValueConstant()`, or
`{"expr": "..."}` when the value cannot be represented; `default_expr` always
carries the stub's own spelling (e.g. `ENT_QUOTES | ENT_SUBSTITUTE |
ENT_HTML401`).
