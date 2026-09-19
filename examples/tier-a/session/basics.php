<?php

// Everything before the first byte of output: php refuses to start a session
// once anything has been sent, so the whole file runs inside a buffer and
// prints it at the end.
ob_start();

var_dump(PHP_SESSION_DISABLED, PHP_SESSION_NONE, PHP_SESSION_ACTIVE);
var_dump(extension_loaded('session'), session_status(), session_id(), session_name());
var_dump(ini_get('session.save_handler'), ini_get('session.serialize_handler'),
    ini_get('session.sid_length'), ini_get('session.sid_bits_per_character'),
    ini_get('session.gc_maxlifetime'), ini_get('session.name'));
var_dump(session_get_cookie_params());

// Nothing works before a session is started.
var_dump(session_write_close(), session_abort(), session_reset(), session_unset(),
    session_encode(), session_decode('a|i:1;'), session_gc(), session_regenerate_id(),
    session_destroy());

// A generated id is `session.sid_length` characters of the alphabet
// `session.sid_bits_per_character` picks.
$made = session_create_id();
var_dump(strlen($made), (bool) preg_match('/^[0-9a-f]{32}$/', $made));
var_dump(session_create_id('pfx-') !== $made, session_create_id('bad!'));

// The lifecycle, on an id of this snippet's choosing so the file is known.
$id = 'tierasession0000000000000000abcd';
var_dump(session_id($id), session_id());
var_dump(session_start(), session_status(), session_id());
var_dump(session_start());

$_SESSION['int'] = 7;
$_SESSION['str'] = 'text';
$_SESSION['arr'] = ['k' => [1, 2]];
$_SESSION['obj'] = new stdClass();
$_SESSION['obj']->p = 'v';
var_dump(session_encode());

$dir = session_save_path() ?: sys_get_temp_dir();
$file = "$dir/sess_$id";
var_dump(session_write_close(), session_status());
var_dump(file_exists($file), file_get_contents($file));

// Reading it back rebuilds every value, objects included.
session_id($id);
var_dump(session_start(), $_SESSION['int'], $_SESSION['arr'], get_class($_SESSION['obj']),
    $_SESSION['obj']->p);
var_dump(session_decode('extra|s:2:"hi";'), $_SESSION['extra']);
var_dump(session_decode('not a session'), session_decode(''));

// A setter is refused while a session is running.
var_dump(session_name('other'), session_save_path('/nope'), session_module_name('files'),
    session_cache_limiter('private'), session_cache_expire(99),
    session_set_cookie_params(['lifetime' => 1]), session_id('another'));

$before = session_id();
var_dump(session_regenerate_id(), session_id() !== $before, strlen(session_id()),
    $_SESSION['int']);
$regenerated = session_id();
var_dump(session_unset(), $_SESSION);
var_dump(session_destroy(), session_status());
var_dump(file_exists("$dir/sess_$regenerated"), file_exists($file));
@unlink($file);

// The settings are writable again once nothing is running.
var_dump(session_name('MYSESSID'), session_name(), ini_get('session.name'));
var_dump(session_cache_limiter('private'), session_cache_limiter(),
    session_cache_expire(99), session_cache_expire());
var_dump(session_module_name(), session_module_name('nope'));
var_dump(session_set_cookie_params(['lifetime' => 60, 'path' => '/app',
    'samesite' => 'Lax', 'secure' => true, 'nonsense' => 1]));
var_dump(session_get_cookie_params(), ini_get('session.cookie_lifetime'),
    ini_get('session.cookie_path'), ini_get('session.cookie_samesite'));
session_name('PHPSESSID');

// A user handler takes over every step.
class TierAHandler implements SessionHandlerInterface
{
    public static array $store = [];
    public static array $calls = [];

    public function open(string $path, string $name): bool
    {
        self::$calls[] = "open($name)";
        return true;
    }

    public function close(): bool
    {
        self::$calls[] = 'close';
        return true;
    }

    public function read(string $id): string|false
    {
        self::$calls[] = "read($id)";
        return self::$store[$id] ?? '';
    }

    public function write(string $id, string $data): bool
    {
        self::$calls[] = "write($id, $data)";
        self::$store[$id] = $data;
        return true;
    }

    public function destroy(string $id): bool
    {
        self::$calls[] = "destroy($id)";
        unset(self::$store[$id]);
        return true;
    }

    public function gc(int $max_lifetime): int|false
    {
        self::$calls[] = "gc($max_lifetime)";
        return 4;
    }
}

var_dump(session_set_save_handler(new TierAHandler()), ini_get('session.save_handler'));
session_id('handler-session');
var_dump(session_start());
$_SESSION['h'] = true;
var_dump(session_gc(), session_write_close(), TierAHandler::$store);
var_dump(session_start(), $_SESSION, session_destroy(), TierAHandler::$store);
var_dump(TierAHandler::$calls);

// The six-callable form is the deprecated one.
var_dump(session_set_save_handler(
    function (string $path, string $name): bool { return true; },
    function (): bool { return true; },
    function (string $id): string { return 'cb|s:8:"callback";'; },
    function (string $id, string $data): bool { return true; },
    function (string $id): bool { return true; },
    function (int $max): int { return 0; }
));
session_id('callback-session');
var_dump(session_start(), $_SESSION, session_write_close());

echo ob_get_clean();
