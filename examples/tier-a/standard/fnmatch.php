<?php
var_dump(FNM_NOESCAPE, FNM_PATHNAME, FNM_PERIOD, FNM_CASEFOLD);
$cases = [
    ['*.txt', 'a.txt'], ['*.txt', 'a.txt.bak'], ['[a-c]x', 'bx'], ['[!a]x', 'bx'], ['[^a]x', 'ax'],
    ['\\*', '*'], ['\\*', 'x'], ['\\*', '\\*', FNM_NOESCAPE], ['*', 'a/b', FNM_PATHNAME], ['*/*', 'a/b', FNM_PATHNAME],
    ['*', '.a', FNM_PERIOD], ['.*', '.a', FNM_PERIOD], ['\\.*', '.a', FNM_PERIOD], ['a/*', 'a/.b', FNM_PERIOD],
    ['a/*', 'a/.b', FNM_PERIOD | FNM_PATHNAME], ['a/.*', 'a/.b', FNM_PERIOD | FNM_PATHNAME],
    ['*a/?b', 'xa/.b', FNM_PERIOD | FNM_PATHNAME], ['a*', 'A', FNM_CASEFOLD], ['A[B-D]', 'ab', FNM_CASEFOLD],
    ['[', '['], ['[a', '[a'], ['[]', '[]'], ['[[]', '['], ['[]]', ']'], ['[!]]', 'a'], ['[a-c-e]', 'd'],
    ['[a-c-e]', '-'], ['[\\]]', ']'], ['[\\\\]', '\\'], ['[b-a]', 'a'], ['a[-z]', 'a-'], ['a[z-]', 'a-'],
    ['[a\\-z]', 'm'], ['[a-\\z]', 'm'], ['a[/]b', 'a/b'], ['a[/]b', 'a/b', FNM_PATHNAME], ['a?b', 'a/b', FNM_PATHNAME],
    ['[[:alpha:]]', 'a'], ['[[:digit:][:alpha:]]', '5'], ['[[:upper:]]', 'a', FNM_CASEFOLD], ['[[:bogus:]]', 'b'],
    ['[[:alpha]', 'a'], ['[[.a.]]', 'a'], ['[[=a=]]', 'a'], ['[![:space:]]', ' '], ['[[:xdigit:]]x', 'fx'],
    ['\\', '\\'], ['a\\', 'a'], ['a\\*', 'a*'], ['', ''], ['', 'a'], ['?', ''], ['*', ''],
    ['a*b*c*d', 'aXXbYYcZZd'], ['a*b*c*d', 'aXXbYYcZZ'], ['**a', 'bba'], ['*', 'a/b', FNM_PATHNAME | 8],
    ['*.php', 'dir/x.php', FNM_PATHNAME], ['*/*.php', 'dir/x.php', FNM_PATHNAME], ["\xe9*", "\xe9t\xe9"], ["?", "\xe9"], ["*", "\xe9"],
];
foreach ($cases as $c) {
    echo json_encode($c[0]), " ~ ", json_encode($c[1]), " flags=", $c[2] ?? 0, ": ";
    var_dump(fnmatch($c[0], $c[1], $c[2] ?? 0));
}
var_dump(fnmatch(str_repeat('x', 1023), str_repeat('x', 1023)));
var_dump(fnmatch(str_repeat('x', 1024), 'x'));
try { fnmatch("a\0", 'a'); } catch (\ValueError $e) { echo $e->getMessage(), "\n"; }
try { fnmatch('a', "a\0"); } catch (\ValueError $e) { echo $e->getMessage(), "\n"; }
try { fnmatch([], 'a'); } catch (\TypeError $e) { echo $e->getMessage(), "\n"; }
var_dump(fnmatch(12, '12'), fnmatch('1*', 123));
