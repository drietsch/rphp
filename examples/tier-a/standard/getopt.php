<?php
// getopt() reads the real command line, so each case runs a second copy of
// the interpreter (PHP_BINARY) on a child script with its own arguments.
file_put_contents('getopt-child.php', <<<'PHP'
<?php
[$short, $long, $byref] = json_decode(file_get_contents('getopt-spec.json'), true);
if ($byref) {
    $rest = null;
    $r = getopt($short, $long, $rest);
    var_export($r);
    echo "\nrest=$rest ", json_encode(array_slice($argv, $rest)), "\n";
} else {
    var_export(getopt($short, $long));
    echo "\n";
}
PHP);

function run(array $args, string $short, array $long = [], bool $byref = true) {
    file_put_contents('getopt-spec.json', json_encode([$short, $long, $byref]));
    $p = proc_open(array_merge([PHP_BINARY, '-n', 'getopt-child.php'], $args), [1 => ['pipe', 'w'], 2 => ['pipe', 'w']], $pipes);
    $out = stream_get_contents($pipes[1]);
    $err = stream_get_contents($pipes[2]);
    fclose($pipes[1]);
    fclose($pipes[2]);
    proc_close($p);
    echo json_encode($args), " ", json_encode($short), " ", json_encode($long), "\n", $out, $err;
}

run(['-a', '-b', 'x'], 'ab');
run(['-ab'], 'ab');
run(['-a', 'v1', '-b'], 'a:b');
run(['-avalue', '-a=eq', '-a', 'sp'], 'a:');
run(['-a'], 'a:');
run(['-a'], 'a::');
run(['-avalue', '-a', 'next'], 'a::');
run(['-a=', '-b'], 'a:b');
run(['-x', '-a'], 'a');
run(['-xa'], 'a');
run(['--foo', '--bar=baz', '--qux', 'quux'], '', ['foo', 'bar:', 'qux:']);
run(['--opt', 'x'], '', ['opt::']);
run(['--opt=x'], '', ['opt::']);
run(['--opt='], '', ['opt:']);
run(['--opt=', 'next'], '', ['opt:']);
run(['--nope', '--foo'], '', ['foo']);
run(['--foo=ignored'], '', ['foo']);
run(['-a', '--', '-b'], 'ab');
run(['-a', '-', '-b'], 'ab');
run(['file', '-a'], 'a');
run(['-a', '-a', '-a'], 'a');
run(['-v', 'x', '-v', 'y'], 'v:');
run(['-1', '-2x', '-9'], '12:9');
run(['--10', '--01'], '', ['10', '01']);
run(['-:'], 'a');
run(['-a'], 'a?b');
run(['-b'], 'a?b');
run(['--f', '--fo'], '', ['foo']);
run([], 'a');
run(['-a', '5'], 'a:');
run(['--arr', 'v'], '', [123 => 'arr:']);
run(['-q', 'rest'], 'q', [], false);
run(['-ab', 'c'], 'a:b');

unlink('getopt-child.php');
unlink('getopt-spec.json');

// In this process the command line is just the script.
var_dump(getopt('abc', ['long']));
$rest = 'x';
var_dump(getopt('a', [], $rest), $rest);
try { getopt('a', 'x'); } catch (\TypeError $e) { echo $e->getMessage(), "\n"; }
