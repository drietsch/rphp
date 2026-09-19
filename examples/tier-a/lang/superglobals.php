<?php
// php's auto-globals are one variable in every scope: no `global`, no capture,
// and a write inside a function is a write to the global.
$_ENV['K'] = 'v';

function readIt(): string { return $_ENV['K'] ?? 'missing'; }
function writeIt(): void { $_ENV['W'] = 'written'; }
function whole(): string { return is_array($_ENV) ? 'array of ' . count($_ENV) : gettype($_ENV); }
function compound(): string { $_ENV += ['C' => 'c']; return $_ENV['C'] ?? 'missing'; }
function nested(): string { $f = function () { return $_ENV['K'] ?? 'missing'; }; return $f(); }
function arrow(): string { $f = fn() => $_ENV['K'] ?? 'missing'; return $f(); }

var_dump(readIt());
writeIt();
var_dump($_ENV['W'] ?? 'missing');
var_dump(whole());
var_dump(compound());
var_dump($_ENV['C'] ?? 'missing');
var_dump(nested(), arrow());

// A local of the same name as a *non*-auto-global is not shared.
$ordinary = 'global';
function shadow(): string { $ordinary = 'local'; return $ordinary; }
var_dump(shadow(), $ordinary);

// $_SERVER is the SAPI's, and a method sees it too.
class C {
    public function script(): bool { return isset($_SERVER['SCRIPT_NAME']) || isset($_SERVER['PHP_SELF']); }
    public static function argvIsArray(): bool { return is_array($_SERVER['argv'] ?? null); }
}
$c = new C();
var_dump($c->script(), C::argvIsArray());

// A superglobal php does not seed springs into existence on write.
function session(): void { $_SESSION['id'] = 42; }
session();
var_dump($_SESSION['id'] ?? 'missing', array_key_exists('_SESSION', $GLOBALS));
