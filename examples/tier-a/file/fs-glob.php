<?php
// `glob()` and its flags as BSD `glob(3)` implements them, which is what php
// exposes on this platform: braces expand left to right and each alternative
// is sorted on its own, `.` and `..` come back from `.*`, `[^a]` is a class
// containing `^` and not a negation, and a pattern that matches nothing is an
// empty array rather than `false`.
$dir = sys_get_temp_dir() . '/rphp-fs-glob';
foreach (@scandir($dir) ?: [] as $e) {
    if ($e !== '.' && $e !== '..') {
        @unlink($dir . '/' . $e);
        @rmdir($dir . '/' . $e);
    }
}
@mkdir($dir);
@mkdir($dir . '/sub');
@mkdir($dir . '/sub2');
foreach (['a.txt', 'b.txt', 'c.dat', '.hidden', 'z'] as $f) {
    file_put_contents($dir . '/' . $f, 'x');
}
@symlink($dir . '/missing', $dir . '/dangling');
chdir($dir);

$show = function (string $label, string $pattern, int $flags = 0): void {
    $r = glob($pattern, $flags);
    echo $label, ': ', $r === false ? 'false' : implode(' ', $r), "\n";
};

$show('all', '*');
$show('txt', '*.txt');
$show('question', '?.txt');
$show('class', '[ab].txt');
$show('range', '[a-c].*');
$show('negate', '[!a].txt');
$show('caret-is-not-a-negation', '[^a].txt');
$show('escaped-dot', 'a\\.txt');
$show('literal-present', 'a.txt');
$show('literal-absent', 'nothinghere');
$show('dangling-symlink', 'dang*');
$show('dot', '.*');
$show('missing-directory', 'nodir/*');
$show('empty-directory', 'sub*/*');
$show('trailing-slash', 'sub*/');
$show('file-trailing-slash', 'a.txt/');
$show('empty-pattern', '');
$show('no-match', 'nomatch*');

$show('mark', '*', GLOB_MARK);
$show('onlydir', '*', GLOB_ONLYDIR);
$show('onlydir+mark', '*', GLOB_ONLYDIR | GLOB_MARK);
$show('nocheck', 'nomatch*', GLOB_NOCHECK);
$show('err', '*', GLOB_ERR);

$show('brace', '*.{txt,dat}', GLOB_BRACE);
$show('brace-keeps-brace-order', '{sub2,a.txt,sub}', GLOB_BRACE);
$show('brace-nested', '{a,{b,c}}.*', GLOB_BRACE);
$show('brace-empty-alternative', 'a{,.txt}', GLOB_BRACE);
$show('brace-unclosed', '{a.txt', GLOB_BRACE);
$show('brace-nocheck', '{nope1,nope2}', GLOB_BRACE | GLOB_NOCHECK);
$show('brace-needs-the-flag', '*.{txt,dat}');

// `GLOB_NOSORT` is readdir order, so only the set is comparable.
$nosort = glob('*', GLOB_NOSORT);
sort($nosort);
echo 'nosort-sorted: ', implode(' ', $nosort), "\n";

// An absolute pattern answers absolute paths.
$abs = glob($dir . '/a.*');
echo 'absolute: ', implode(' ', array_map('basename', $abs)), ' ', count($abs), "\n";

var_dump(GLOB_ERR, GLOB_MARK, GLOB_NOCHECK, GLOB_NOSORT, GLOB_BRACE, GLOB_NOESCAPE, GLOB_ONLYDIR);
var_dump(GLOB_AVAILABLE_FLAGS === (GLOB_ERR | GLOB_MARK | GLOB_NOCHECK | GLOB_NOSORT | GLOB_BRACE | GLOB_NOESCAPE | GLOB_ONLYDIR));

// A flag the platform does not have is the one way `glob()` answers `false`.
var_dump(@glob('*', 1), error_get_last()['message']);

chdir(sys_get_temp_dir());
foreach (['a.txt', 'b.txt', 'c.dat', '.hidden', 'z', 'dangling'] as $f) {
    unlink($dir . '/' . $f);
}
var_dump(rmdir($dir . '/sub'), rmdir($dir . '/sub2'), rmdir($dir));
