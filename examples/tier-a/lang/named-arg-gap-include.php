<?php
// A named argument that skips optional parameters must still leave those
// parameters at their defaults — also when the function has a symbol table
// (it contains an `include`, so php materialises its variables by name).
// Symfony's Dotenv::bootEnv($file, overrideExistingVars: true) is this shape.
function boot(string $path, string $defaultEnv = 'dev', array $testEnvs = ['test'], bool $overrideExistingVars = false, $tail = null): string
{
    if ($path === '/nonexistent') {
        $extra = include __DIR__ . '/no-such-file-for-named-arg-gap.php';
    }
    return $defaultEnv . '|' . implode(',', $testEnvs) . '|' . var_export($overrideExistingVars, true) . '|' . var_export($tail, true);
}

function plain(string $path, string $defaultEnv = 'dev', int $n = 1, bool $flag = false): string
{
    return $defaultEnv . '|' . $n . '|' . var_export($flag, true);
}

echo boot('/x', overrideExistingVars: true), "\n";
echo plain('/x', flag: true), "\n";
echo boot('/x', tail: 'T', defaultEnv: 'prod'), "\n";
echo boot('/x', testEnvs: ['a', 'b']), "\n";
