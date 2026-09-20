<?php

// A `Stringable` object reaching a native's `string` parameter is its
// `__toString()` — php's argument parser converts it, whether the parameter
// is `string`, `?string`, `array|string` or `string|int`, and leaves it an
// object for `mixed`, `object` and a class type.
class S
{
    public function __construct(private string $v = 'stringy') {}
    public function __toString(): string { return $this->v; }
}
class NotStringable {}
$s = new S();
var_dump(strlen($s), strtoupper($s), str_pad($s, 10, '-'), substr($s, 0, 3), strpos($s, 'ing'),
    trim($s), ucfirst($s), str_replace('str', 'STR', $s), explode('i', $s),
    htmlspecialchars($s), md5($s) === md5('stringy'), strrev($s), str_contains($s, 'ring'),
    array_map('strtoupper', [$s]), preg_match('/str/', $s), preg_replace('/i/', 'I', $s),
    mb_strlen($s), iconv('UTF-8', 'ASCII', $s), hash('crc32b', $s) === hash('crc32b', 'stringy'),
    json_encode(['k' => $s]), base64_encode($s), urlencode($s), (new DateTime('2020-01-01'))->format(new S('Y')));

// The natives with `mixed` parameters convert on their own: `%s`, `implode()`,
// and a loose comparison with a string.
var_dump(sprintf('%s|%5s|%-9s|%d', $s, $s, $s, '3'), implode(',', [$s, 1, $s]), join('', [$s]));
var_dump("x" == $s, 'stringy' == $s, $s == 'stringy', in_array('stringy', [$s]), array_search('stringy', [1, $s]),
    $s == new NotStringable(), 'x' == new NotStringable(), new NotStringable() == 'x');
printf("%s\n", $s);

// One with no `__toString()` is refused by the native, with php's message.
foreach ([fn() => strlen(new NotStringable()), fn() => sprintf('%s', new NotStringable()),
          fn() => implode(',', [new NotStringable()]), fn() => str_replace('a', 'b', new NotStringable())] as $f) {
    try {
        $f();
        echo "no throw\n";
    } catch (Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}

// And a parameter that takes an object keeps it.
var_dump(get_class($s), is_object($s), is_string($s), gettype($s), spl_object_id($s) > 0,
    in_array($s, [$s], true), array_search($s, [$s], true), count([$s]));
