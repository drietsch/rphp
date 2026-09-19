<?php
// `new $cls` autoloads exactly as `new A` does, and the loader is called with
// the name stripped of a leading `\` while the error keeps the spelling.
spl_autoload_register(function (string $c): void {
    echo "autoload: $c\n";
    if ($c === 'Made') {
        eval('class Made { public function who(): string { return "made"; } }');
    }
});

$n = 'Made';
$o = new $n();
var_dump($o->who(), get_class($o));

$b = '\Made';
var_dump(get_class(new $b()));

// `instanceof` with a string never autoloads: an unknown class just does not
// match.
$missing = 'NeverLoaded';
var_dump($o instanceof $missing);

try {
    new $missing();
} catch (Error $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

// An object on the right of `new` uses its class.
$c = new ($o::class)();
var_dump(get_class($c));
