<?php
// php warns on every *read* of a variable that has never been assigned, and
// the value it yields is null. A quiet read — `??`, `isset`, `empty` — never
// warns, and `@` suppresses.
function report(): void {
    echo "---\n";
}

$a = 1;
echo $a, "\n";
var_dump($undefined);
var_dump($undefined);          // warns again: the read is what warns
var_dump($undefined ?? 'fallback', isset($undefined), empty($undefined));
var_dump(@$undefined);

// A conditional assignment is not an assignment the compiler can rely on.
function maybe(bool $yes): void {
    if ($yes) {
        $set = 'yes';
    }
    var_dump($set ?? 'unset');
    echo $set;
    echo "\n";
}
maybe(true);
maybe(false);

// Parameters, `use` captures, `global`, `static`, `foreach` targets, `catch`
// variables, destructuring and reference bindings all count as assigned.
function params(int $p, string $q = 'd'): string { return $p . $q; }
var_dump(params(1));
$cap = 'captured';
var_dump((function () use ($cap) { return $cap; })());
function usesGlobal(): string { global $g; return $g; }
$g = 'global';
var_dump(usesGlobal());
function counter(): int { static $n = 0; return ++$n; }
var_dump(counter(), counter());
foreach (['k' => 'v'] as $key => $value) {
}
var_dump($key, $value);
try {
    throw new LogicException('caught');
} catch (LogicException $e) {
    var_dump($e->getMessage());
}
[$one, $two] = [1, 2];
var_dump($one, $two);
$target = 'x';
$alias = &$target;
var_dump($alias);

// The loop body may never run.
foreach ([] as $none) {
}
var_dump($none ?? 'never assigned');

// An undefined variable in a string interpolation, and a quiet element read
// whose key is one.
$arr = ['k' => 1];
var_dump("[$missing]");
var_dump($arr['nope'] ?? 'no key');
