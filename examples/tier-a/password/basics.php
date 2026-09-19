<?php
// Differential coverage for the password / crypt extension.
//
// Every hash the password API writes carries a random salt, so nothing that
// comes out of password_hash() is ever printed here. The exact-output cases go
// through crypt() with a fixed setting; the random-salt ones are checked with
// password_verify(), password_get_info() and password_needs_rehash(), which
// answer in booleans and in parameters read back out of the string.
//
// Passwords stay 7-bit throughout: $2a$ and $2x$ only diverge from $2y$ for
// certain 8-bit passwords, which this extension deliberately refuses rather
// than answering wrongly.

// ---- constants ---------------------------------------------------------
// php 8.0 turned the algorithm identifiers into strings; the tuning values
// stayed ints, and the bcrypt default cost became 12 in 8.4.
echo "-- constants\n";
var_dump(PASSWORD_DEFAULT);
var_dump(PASSWORD_BCRYPT);
var_dump(PASSWORD_ARGON2I);
var_dump(PASSWORD_ARGON2ID);
var_dump(PASSWORD_BCRYPT_DEFAULT_COST);
var_dump(PASSWORD_ARGON2_DEFAULT_MEMORY_COST);
var_dump(PASSWORD_ARGON2_DEFAULT_TIME_COST);
var_dump(PASSWORD_ARGON2_DEFAULT_THREADS);
var_dump(PASSWORD_ARGON2_PROVIDER);
var_dump(PASSWORD_DEFAULT === PASSWORD_BCRYPT);
var_dump(password_algos());

// ---- crypt(): the scheme table -----------------------------------------
// Each setting is reproduced in the output, so these are byte-exact vectors.
echo "-- crypt schemes\n";
$settings = [
    '$2y$10$usesomesillystringforsalt',
    '$2a$10$usesomesillystringforsalt',
    '$2b$10$usesomesillystringforsalt',
    '$2x$10$usesomesillystringforsalt',
    '$2y$04$usesomesillystringforsalt',
    '$1$usesomes$',
    '$1$usesomes',
    '$1$usesomesillystring$',
    '$5$usesomesillystringforsalt$',
    '$6$usesomesillystringforsalt$',
    '$5$rounds=5000$usesomesillystri$',
    '$6$rounds=1000$usesomesillystri$',
    '$6$rounds=+5000$usesomes$',
    '$6$rounds=0005000$usesomes$',
    'rl',
    '..',
    '/.',
    'zz',
    '_J9..rasm',
    '_J9..rasmBYk8r9AiWNc',
    '_zzzzabcd',
];
foreach ($settings as $s) {
    echo $s, ' => ', crypt('rasmuslerdorf', $s), "\n";
}

// A 22-character bcrypt salt is 16 bytes of base64, so its last character only
// carries two significant bits and comes back normalised: "...fors" -> "...fore".
echo "-- bcrypt salt normalisation\n";
foreach (['usesomesillystringfors', '......................', 'AAAAAAAAAAAAAAAAAAAAAA', 'zzzzzzzzzzzzzzzzzzzzzz'] as $salt) {
    echo $salt, ' => ', crypt('rasmuslerdorf', '$2y$04$' . $salt), "\n";
}

// bcrypt reads at most 72 bytes of password, and both arguments stop at a NUL.
echo "-- truncation\n";
var_dump(crypt(str_repeat('a', 72), '$2y$04$usesomesillystringforsalt') === crypt(str_repeat('a', 100), '$2y$04$usesomesillystringforsalt'));
var_dump(crypt(str_repeat('a', 71), '$2y$04$usesomesillystringforsalt') === crypt(str_repeat('a', 72), '$2y$04$usesomesillystringforsalt'));
var_dump(crypt("a\0b", '$2y$04$usesomesillystringforsalt') === crypt('a', '$2y$04$usesomesillystringforsalt'));
var_dump(crypt("a\0b", '$1$usesomes$') === crypt('a', '$1$usesomes$'));
var_dump(crypt('x', "\$1\$ab\0cd\$") === crypt('x', '$1$ab$'));
// The $1$ salt stops at 8 characters, the $5$/$6$ salt at 16.
var_dump(crypt('x', '$1$abcdefghij$') === crypt('x', '$1$abcdefgh$'));
var_dump(crypt('x', '$6$0123456789abcdefg$') === crypt('x', '$6$0123456789abcdef$'));

// A rounds= that is not followed by a '$' after its digits is not a round
// count at all, but literal salt text.
echo "-- rounds= parsing\n";
var_dump(crypt('x', '$6$rounds=abc$usesomes$') === crypt('x', '$6$rounds=abc$'));
var_dump(crypt('x', '$6$rounds=5000$usesomes$') === crypt('x', '$6$usesomes$'));

// ---- crypt(): failure --------------------------------------------------
// A setting crypt() cannot use yields "*0" — or "*1" when the setting already
// began with "*0", so the answer can never equal its own input.
echo "-- crypt failure tokens\n";
$bad = [
    '', 'x', '$', '$$', '$9$abc$', '$2y$', '$2y$10$', '$2y$10$short',
    '$2y$03$usesomesillystringforsalt', '$2y$32$usesomesillystringforsalt',
    '$2z$10$usesomesillystringforsalt', '$2$10$usesomesillystringforsalt',
    '$2y$4$usesomesillystringforsalt', '$2y$004$usesomesillystringforsalt',
    '$2y$1a$usesomesillystringforsalt', '$2y$04$usesomesillystringfor!!',
    '$6$rounds=999$usesomesillystri$', '$6$rounds=1000000000$x$',
    '$6$rounds=$usesomes$', '$6$rounds=-1$usesomes$',
    '!!', 'a!', '$$', 'a', '*', '*1', '_', '_....abcd',
];
foreach ($bad as $s) {
    echo $s, ' => ', crypt('rasmuslerdorf', $s), "\n";
}
// The one setting that answers "*1".
var_dump(crypt('rasmuslerdorf', '*0'));

// ---- password_get_info() -----------------------------------------------
// Three keys, always in this order. Only a 60-byte string starting "$2y" is
// recognised as bcrypt, so $2a$/$2b$/$2x$ read as unknown even though crypt()
// and password_verify() handle them.
echo "-- password_get_info\n";
$hashes = [
    '$2y$04$usesomesillystringforeqLFqxxK4dcfWQAiMrJd.1JLzAd4yJVG',
    '$2y$12$usesomesillystringforeqLFqxxK4dcfWQAiMrJd.1JLzAd4yJVG',
    '$2a$04$usesomesillystringforeqLFqxxK4dcfWQAiMrJd.1JLzAd4yJVG',
    '$2b$04$usesomesillystringforeqLFqxxK4dcfWQAiMrJd.1JLzAd4yJVG',
    '$2y$xx$usesomesillystringforeqLFqxxK4dcfWQAiMrJd.1JLzAd4yJVG',
    '$argon2i$v=19$m=65536,t=4,p=1$d2lFZWUzV1hQNXhPdFdxZA$k6qx+SUe92pGvRilT2RHo75n3dOh9YMGbb1TuyYhjLg',
    '$argon2id$v=19$m=32,t=2,p=1$c3VTUVBmTWN0Lm83M3dURQ$7Z3B9YRelGgF3lIZh0hajy/qi3h4d6XWJ024bwGSyfA',
    '$argon2i$',
    '$argon2id$v=19$m=65536,t=4,p=1$',
    '$1$usesomes$dftn4s4hqGa1pDUjES2GI.',
    'rl.3StKT.4T8M',
    'not-a-hash',
    '',
];
foreach ($hashes as $h) {
    echo json_encode(password_get_info($h)), "\n";
}
// The 59th byte matters: one character short and it is no longer bcrypt.
echo json_encode(password_get_info(substr('$2y$04$usesomesillystringforeqLFqxxK4dcfWQAiMrJd.1JLzAd4yJVG', 0, 59))), "\n";

// ---- password_verify() -------------------------------------------------
// Anything that is not argon2 is verified by re-running crypt() with the
// stored hash as the setting, which is why every legacy scheme works.
echo "-- password_verify against fixed hashes\n";
var_dump(password_verify('rasmuslerdorf', '$2y$04$usesomesillystringforeSZ3saLXGnQtLrpxgqydzBYMhY/4iDpG'));
var_dump(password_verify('wrong', '$2y$04$usesomesillystringforeSZ3saLXGnQtLrpxgqydzBYMhY/4iDpG'));
var_dump(password_verify('rasmuslerdorf', '$2a$04$usesomesillystringforeSZ3saLXGnQtLrpxgqydzBYMhY/4iDpG'));
var_dump(password_verify('rasmuslerdorf', '$2b$04$usesomesillystringforeSZ3saLXGnQtLrpxgqydzBYMhY/4iDpG'));
var_dump(password_verify('rasmuslerdorf', '$1$usesomes$dftn4s4hqGa1pDUjES2GI.'));
var_dump(password_verify('rasmuslerdorf', '$5$usesomesillystri$KqJWpanXZHKq2BOB43TSaYhEWsQ1Lr5QNyPCDH/Tp.6'));
var_dump(password_verify('rasmuslerdorf', 'rl.3StKT.4T8M'));
var_dump(password_verify('rasmuslerdorf', '_J9..rasmBYk8r9AiWNc'));
var_dump(password_verify('rasmuslerdorf', ''));
var_dump(password_verify('rasmuslerdorf', 'garbage'));
var_dump(password_verify('rasmuslerdorf', '*0'));
var_dump(password_verify('x', '$argon2i$'));
var_dump(password_verify('x', '$argon2id$v=19$m=65536,t=4,p=1$'));

// ---- password_hash() round trips ---------------------------------------
// The hashes themselves are random; only their shape and their answers are
// printed.
echo "-- password_hash round trips\n";
$bc = password_hash('rasmuslerdorf', PASSWORD_BCRYPT, ['cost' => 4]);
var_dump(strlen($bc), substr($bc, 0, 7));
var_dump(password_verify('rasmuslerdorf', $bc), password_verify('wrong', $bc));
var_dump(password_get_info($bc)['algo'], password_get_info($bc)['algoName']);
var_dump(password_get_info($bc)['options']['cost']);

$a2i = password_hash('rasmuslerdorf', PASSWORD_ARGON2I, ['memory_cost' => 64, 'time_cost' => 1, 'threads' => 1]);
var_dump(substr($a2i, 0, 26));
var_dump(password_verify('rasmuslerdorf', $a2i), password_verify('wrong', $a2i));
var_dump(json_encode(password_get_info($a2i)));

$a2id = password_hash('rasmuslerdorf', PASSWORD_ARGON2ID, ['memory_cost' => 64, 'time_cost' => 1, 'threads' => 1]);
var_dump(substr($a2id, 0, 27));
var_dump(password_verify('rasmuslerdorf', $a2id), password_verify('wrong', $a2id));
var_dump(json_encode(password_get_info($a2id)));

// The legacy integer spellings still resolve: 0 and null mean the default,
// 1 bcrypt, 2 argon2i, 3 argon2id.
echo "-- legacy algo spellings\n";
var_dump(substr(password_hash('x', 1, ['cost' => 4]), 0, 4));
var_dump(substr(password_hash('x', 2, ['memory_cost' => 64, 'time_cost' => 1, 'threads' => 1]), 0, 9));
var_dump(substr(password_hash('x', 3, ['memory_cost' => 64, 'time_cost' => 1, 'threads' => 1]), 0, 10));
var_dump(substr(password_hash('x', 0, ['cost' => 4]), 0, 4));
var_dump(substr(password_hash('x', null, ['cost' => 4]), 0, 4));
// A 72-byte cut-off applies here too, silently.
$long = password_hash(str_repeat('a', 100), PASSWORD_BCRYPT, ['cost' => 4]);
var_dump(password_verify(str_repeat('a', 100), $long), password_verify(str_repeat('a', 72), $long));

// ---- password_needs_rehash() -------------------------------------------
// It never validates its options, and an algorithm it does not know is never
// worth rehashing to.
echo "-- password_needs_rehash\n";
$b4 = '$2y$04$usesomesillystringforeqLFqxxK4dcfWQAiMrJd.1JLzAd4yJVG';
$ai = '$argon2i$v=19$m=65536,t=4,p=1$d2lFZWUzV1hQNXhPdFdxZA$k6qx+SUe92pGvRilT2RHo75n3dOh9YMGbb1TuyYhjLg';
var_dump(password_needs_rehash($b4, PASSWORD_BCRYPT));
var_dump(password_needs_rehash($b4, PASSWORD_BCRYPT, ['cost' => 4]));
var_dump(password_needs_rehash($b4, PASSWORD_DEFAULT));
var_dump(password_needs_rehash($b4, PASSWORD_ARGON2I));
var_dump(password_needs_rehash($b4, 1, ['cost' => 4]));
var_dump(password_needs_rehash($b4, PASSWORD_BCRYPT, ['cost' => 99]));
var_dump(password_needs_rehash($b4, 'nope'));
var_dump(password_needs_rehash($ai, PASSWORD_ARGON2I));
var_dump(password_needs_rehash($ai, PASSWORD_ARGON2I, ['memory_cost' => 65536, 'time_cost' => 4, 'threads' => 1]));
var_dump(password_needs_rehash($ai, PASSWORD_ARGON2I, ['time_cost' => 5]));
var_dump(password_needs_rehash($ai, PASSWORD_ARGON2ID));
var_dump(password_needs_rehash('garbage', PASSWORD_BCRYPT));
var_dump(password_needs_rehash('', PASSWORD_BCRYPT));

// ---- errors ------------------------------------------------------------
echo "-- errors\n";
function attempt(string $label, callable $f): void
{
    try {
        $f();
        echo $label, ' => ok', "\n";
    } catch (Throwable $e) {
        echo $label, ' => ', get_class($e), ': ', $e->getMessage(), "\n";
    }
}

attempt('cost 3', function () { password_hash('x', PASSWORD_BCRYPT, ['cost' => 3]); });
attempt('cost 32', function () { password_hash('x', PASSWORD_BCRYPT, ['cost' => 32]); });
attempt('cost 0', function () { password_hash('x', PASSWORD_BCRYPT, ['cost' => 0]); });
attempt('cost -1', function () { password_hash('x', PASSWORD_BCRYPT, ['cost' => -1]); });
attempt('cost abc', function () { password_hash('x', PASSWORD_BCRYPT, ['cost' => 'abc']); });
attempt('cost 4', function () { password_hash('x', PASSWORD_BCRYPT, ['cost' => 4]); });
attempt('cost "4"', function () { password_hash('x', PASSWORD_BCRYPT, ['cost' => '4']); });
attempt('nul password', function () { password_hash("a\0b", PASSWORD_BCRYPT, ['cost' => 4]); });
attempt('nul beats cost', function () { password_hash("a\0b", PASSWORD_BCRYPT, ['cost' => 3]); });
attempt('algo nope', function () { password_hash('x', 'nope'); });
attempt('algo 9999', function () { password_hash('x', 9999); });
attempt('algo ARGON2I', function () { password_hash('x', 'ARGON2I'); });
attempt('algo bcrypt', function () { password_hash('x', 'bcrypt'); });
attempt('options null', function () { password_hash('x', PASSWORD_BCRYPT, null); });
attempt('options null beats algo', function () { password_hash('x', 'nope', null); });
attempt('needs_rehash options null', function () { password_needs_rehash('x', PASSWORD_BCRYPT, null); });
attempt('memory_cost 7', function () { password_hash('x', PASSWORD_ARGON2I, ['memory_cost' => 7]); });
attempt('memory_cost 0', function () { password_hash('x', PASSWORD_ARGON2I, ['memory_cost' => 0]); });
attempt('memory_cost -1', function () { password_hash('x', PASSWORD_ARGON2I, ['memory_cost' => -1]); });
attempt('memory_cost 2^32', function () { password_hash('x', PASSWORD_ARGON2I, ['memory_cost' => 4294967296, 'time_cost' => 1, 'threads' => 1]); });
attempt('time_cost 0', function () { password_hash('x', PASSWORD_ARGON2I, ['time_cost' => 0]); });
attempt('time_cost -1', function () { password_hash('x', PASSWORD_ARGON2I, ['time_cost' => -1]); });
attempt('time_cost 2^32', function () { password_hash('x', PASSWORD_ARGON2I, ['memory_cost' => 64, 'time_cost' => 4294967296, 'threads' => 1]); });
attempt('threads 0', function () { password_hash('x', PASSWORD_ARGON2I, ['threads' => 0]); });
attempt('threads -1', function () { password_hash('x', PASSWORD_ARGON2I, ['threads' => -1]); });
attempt('threads 2^24', function () { password_hash('x', PASSWORD_ARGON2I, ['memory_cost' => 64, 'time_cost' => 1, 'threads' => 16777216]); });
attempt('m=8 p=2', function () { password_hash('x', PASSWORD_ARGON2I, ['memory_cost' => 8, 'time_cost' => 1, 'threads' => 2]); });
attempt('m=15 p=2', function () { password_hash('x', PASSWORD_ARGON2I, ['memory_cost' => 15, 'time_cost' => 1, 'threads' => 2]); });
attempt('m=16 p=2', function () { password_hash('x', PASSWORD_ARGON2I, ['memory_cost' => 16, 'time_cost' => 1, 'threads' => 2]); });
attempt('crypt 1 arg', function () { crypt('abc'); });

// The "salt" option has been ignored since php 7 and removed in 8; the warning
// it raises carries a file path, so it is suppressed and only the result of
// having passed it is observed.
echo "-- ignored salt option\n";
$salted = @password_hash('rasmuslerdorf', PASSWORD_BCRYPT, ['cost' => 4, 'salt' => 'usesomesillystringfo']);
var_dump(strlen($salted), substr($salted, 0, 7));
var_dump(password_verify('rasmuslerdorf', $salted));
// An unrecognised option is simply ignored.
var_dump(substr(password_hash('x', PASSWORD_BCRYPT, ['cost' => 4, 'bogus' => 1]), 0, 7));
