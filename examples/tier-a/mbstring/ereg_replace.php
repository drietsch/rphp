<?php
// mb_ereg_replace / mb_eregi_replace / mb_ereg_replace_callback: the
// replacement grammar (\0-\9, \k<name>, \k'name', what stays literal),
// php's empty-match loop (one byte past an empty match, even inside a
// UTF-8 character), options strings, invalid subjects.
foreach (['\k', '\k<', '\k<1', "a\\", '\\\\1', '\k<>', "\\k'1'", '\k<01>', '\k<10>', '\9', '\0\0', 'é\1', "\\é",
          '[\0|\1|\2|\3|\\\\1|\k<1>|\k<01>|\k<x>|\k|\q\\'] as $r) {
    echo json_encode($r), " => ", bin2hex(mb_ereg_replace('(a)(b)?', $r, 'xax ab')), "\n";
}
var_dump(mb_ereg_replace('(?<x>a)(b)?', '[\0|\1|\k<x>|\k\'x\'|\k<1>|\k<y>]', 'a ab'));
var_dump(mb_ereg_replace('(?<x>a)', '[\k<x', 'a'));
var_dump(mb_ereg_replace('(?<w>\w)', '<\k<w>|\1|\k<1>>', 'ab'));

echo "-- empty matches\n";
var_dump(bin2hex(mb_ereg_replace('', '-', 'äb')));
var_dump(bin2hex(mb_ereg_replace('b*', '-', 'äbc')));
var_dump(mb_ereg_replace('^', 'X', "a\nb\n", 'r'));
var_dump(mb_ereg_replace('$', 'X', "a\nb\n", 'r'));
var_dump(mb_ereg_replace('^', 'X', "a\nb\n", 'p'));
var_dump(mb_ereg_replace('$', 'X', "a\nb\n", 'p'));
var_dump(mb_ereg_replace('\Z', 'X', "a\nb\n"));
var_dump(mb_ereg_replace('x*', '-', 'ab', 'n'), mb_ereg_replace('x', 'y', 'axb', 'n'));

echo "-- options\n";
var_dump(mb_ereg_replace('a', 'x', 'AaA', 'i'), mb_eregi_replace('a', 'x', 'AaA'), mb_ereg_replace('a', 'x', 'AaA', ''));
var_dump(mb_eregi_replace('straße', 'X', 'STRASSE Straße'));
var_dump(mb_ereg_replace('(?x) a  # comment
  b', 'X', 'ab a b'));
var_dump(mb_ereg_replace(' -', '+', '- - - - -', 'x'));
var_dump(mb_ereg_replace('\(a\)', '[\1]', 'a(a)', 'b'));
var_dump(mb_ereg_replace('a\{2\}', 'X', 'aaa', 'g'));

echo "-- callback\n";
var_dump(mb_ereg_replace_callback('(a)(b)?', function ($m) { var_dump($m); return 'X'; }, 'a ab'));
var_dump(mb_ereg_replace_callback('(?<n>a)(?<o>b)?', fn($m) => json_encode($m), 'ab a'));
var_dump(mb_ereg_replace_callback('\d', fn($m) => $m[0] * 2, 'a1b2c3'));
try {
    mb_ereg_replace_callback('a', function () { throw new \Exception('boom'); }, 'a');
} catch (\Exception $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}

echo "-- invalid subject\n";
var_dump(mb_ereg_replace("a", "b", "\xff"), mb_eregi_replace("a", "b", "\xff"));
