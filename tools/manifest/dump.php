<?php
/**
 * Dump the installed PHP's internal surface (functions, classes, constants,
 * ini directives, extensions) as deterministic JSON via Reflection.
 *
 *   php -n tools/manifest/dump.php manifest/php-8.5.0
 *
 * Run with `-n` so no php.ini leaks into the dump: `ini.json` then holds the
 * compiled-in defaults and the extension set is the statically linked one.
 * Runtime-computed constants (PHP_BINARY, PHP_OS, STDIN, …) are environment
 * specific; `cargo xtask gen` maps them to `ConstValue::Runtime`.
 *
 * Class entries list only the members the class itself declares (constants,
 * properties, methods and enum cases as lists in declaration order, since that
 * order is observable through get_class_methods()/ReflectionEnum::getCases());
 * inherited members are reachable through `parent` / `all_interfaces`.
 *
 * Output (all maps key-sorted, lists in declaration order):
 *   functions.json   { name: Fn }             every get_defined_functions()['internal']
 *   classes.json     { Name: Class }          every internal class/interface/trait/enum
 *   constants.json   { category: { NAME: Const } }   get_defined_constants(true)
 *   ini.json         { name: Ini }            ini_get_all(null, true) + owning extension
 *   extensions.json  { name: Ext }            get_loaded_extensions()
 */
declare(strict_types=1);

error_reporting(E_ALL & ~E_DEPRECATED);

if ($argc < 2) {
    fwrite(STDERR, "usage: php -n tools/manifest/dump.php <out-dir>\n");
    exit(2);
}
$outDir = rtrim($argv[1], '/');
if (!is_dir($outDir) && !mkdir($outDir, 0o777, true)) {
    fwrite(STDERR, "cannot create $outDir\n");
    exit(1);
}

// ---------------------------------------------------------------------------
// helpers

/** Key-sorted map (emitted as a JSON object even when empty). */
function m(array $a): stdClass
{
    ksort($a, SORT_STRING);
    return (object) $a;
}

/** Canonical PHP type name of a value (`gettype` spelled the modern way). */
function typeName(mixed $v): string
{
    return match (gettype($v)) {
        'NULL' => 'null',
        'boolean' => 'bool',
        'integer' => 'int',
        'double' => 'float',
        'string' => 'string',
        'array' => 'array',
        'object' => 'object',
        default => 'resource',
    };
}

/** A PHP value in JSON-safe form; non-representable values become tagged objects. */
function val(mixed $v): mixed
{
    if ($v === null || is_bool($v) || is_int($v)) {
        return $v;
    }
    if (is_float($v)) {
        if (is_nan($v)) {
            return (object) ['expr' => 'NAN'];
        }
        if (is_infinite($v)) {
            return (object) ['expr' => $v > 0 ? 'INF' : '-INF'];
        }
        return $v;
    }
    if (is_string($v)) {
        return preg_match('//u', $v) === 1 ? $v : (object) ['bytes' => base64_encode($v)];
    }
    if (is_array($v)) {
        if (array_is_list($v)) {
            return array_map('val', $v);
        }
        return m(array_map('val', $v));
    }
    if ($v instanceof UnitEnum) {
        return (object) ['enum' => get_class($v) . '::' . $v->name];
    }
    if (is_object($v)) {
        return (object) ['object' => get_class($v)];
    }
    return (object) ['resource' => get_resource_type($v)];
}

function typeInfo(?ReflectionType $t): ?stdClass
{
    if ($t === null) {
        return null;
    }
    return (object) ['type' => (string) $t, 'nullable' => $t->allowsNull()];
}

function attrs(ReflectionFunctionAbstract|ReflectionClass|ReflectionClassConstant|ReflectionProperty $r): array
{
    $out = [];
    foreach ($r->getAttributes() as $a) {
        $out[] = (object) ['name' => $a->getName(), 'args' => val($a->getArguments())];
    }
    return $out;
}

/** The raw default-value text of an internal parameter, from its stub arginfo. */
function defaultExpr(ReflectionParameter $p): ?string
{
    if (preg_match('/ = (.*) \]$/s', (string) $p, $mm) === 1) {
        return $mm[1];
    }
    return null;
}

function param(ReflectionParameter $p): stdClass
{
    $byRef = $p->isPassedByReference();
    $e = [
        'name' => $p->getName(),
        'type' => $p->hasType() ? (string) $p->getType() : null,
        'nullable' => $p->allowsNull(),
        'by_ref' => $byRef,
        'prefer_ref' => $byRef && $p->canBePassedByValue(),
        'variadic' => $p->isVariadic(),
        'optional' => $p->isOptional(),
    ];
    if ($p->isDefaultValueAvailable()) {
        $expr = defaultExpr($p);
        if ($expr !== null) {
            $e['default_expr'] = $expr;
        }
        if ($p->isDefaultValueConstant()) {
            $e['default'] = (object) ['const' => $p->getDefaultValueConstantName()];
        } else {
            try {
                $v = $p->getDefaultValue();
                $e['default'] = is_object($v) ? (object) ['expr' => $expr ?? get_class($v)] : val($v);
            } catch (Throwable) {
                $e['default'] = (object) ['expr' => $expr ?? '?'];
            }
        }
    }
    return (object) $e;
}

function sig(ReflectionFunctionAbstract $f): array
{
    $ret = $f->getReturnType();
    $tentative = false;
    if ($ret === null && $f instanceof ReflectionMethod && $f->hasTentativeReturnType()) {
        $ret = $f->getTentativeReturnType();
        $tentative = true;
    }
    $r = typeInfo($ret);
    if ($r !== null && $f instanceof ReflectionMethod) {
        $r->tentative = $tentative;
    }
    $e = [
        'name' => $f->getName(),
        'deprecated' => $f->isDeprecated(),
        'returns_ref' => $f->returnsReference(),
        'required' => $f->getNumberOfRequiredParameters(),
        'return' => $r,
        'params' => array_map('param', $f->getParameters()),
    ];
    $a = attrs($f);
    if ($a !== []) {
        $e['attributes'] = $a;
    }
    return $e;
}

function visibility(ReflectionMethod|ReflectionProperty|ReflectionClassConstant $r): string
{
    return $r->isPrivate() ? 'private' : ($r->isProtected() ? 'protected' : 'public');
}

// ---------------------------------------------------------------------------
// functions

$functions = [];
foreach (get_defined_functions()['internal'] as $name) {
    $f = new ReflectionFunction($name);
    $e = sig($f);
    $e['extension'] = $f->getExtensionName() ?: 'Core';
    $functions[$f->getName()] = (object) $e;
}

// ---------------------------------------------------------------------------
// classes

$classes = [];
$names = array_merge(get_declared_classes(), get_declared_interfaces(), get_declared_traits());
foreach ($names as $name) {
    $c = new ReflectionClass($name);
    if (!$c->isInternal()) {
        continue;
    }
    $kind = $c->isEnum() ? 'enum' : ($c->isInterface() ? 'interface' : ($c->isTrait() ? 'trait' : 'class'));
    $all = $c->getInterfaceNames();
    $inherited = [];
    if ($c->getParentClass() !== false) {
        $inherited = $c->getParentClass()->getInterfaceNames();
    }
    foreach ($all as $i) {
        $inherited = array_merge($inherited, (new ReflectionClass($i))->getInterfaceNames());
    }
    $direct = array_values(array_diff($all, $inherited));

    $consts = [];
    foreach ($c->getReflectionConstants() as $k) {
        if ($k->isEnumCase() || $k->getDeclaringClass()->getName() !== $c->getName()) {
            continue;
        }
        $consts[] = (object) [
            'name' => $k->getName(),
            'value' => val($k->getValue()),
            'visibility' => visibility($k),
            'final' => $k->isFinal(),
            'deprecated' => $k->isDeprecated(),
            'type' => $k->getType() !== null ? (string) $k->getType() : null,
        ];
    }

    $props = [];
    foreach ($c->getProperties() as $p) {
        if ($p->getDeclaringClass()->getName() !== $c->getName()) {
            continue;
        }
        $pe = [
            'name' => $p->getName(),
            'static' => $p->isStatic(),
            'visibility' => visibility($p),
            'set_visibility' => $p->isPrivateSet() ? 'private' : ($p->isProtectedSet() ? 'protected' : null),
            'readonly' => $p->isReadOnly(),
            'type' => $p->hasType() ? (string) $p->getType() : null,
            'nullable' => $p->hasType() ? $p->getType()->allowsNull() : true,
            'has_default' => $p->hasDefaultValue(),
            'default' => $p->hasDefaultValue() ? val($p->getDefaultValue()) : null,
            'virtual' => $p->isVirtual(),
        ];
        if ($p->hasHooks()) {
            $pe['hooks'] = array_keys($p->getHooks());
        }
        $props[] = (object) $pe;
    }

    $methods = [];
    foreach ($c->getMethods() as $mth) {
        if ($mth->getDeclaringClass()->getName() !== $c->getName()) {
            continue;
        }
        $me = sig($mth);
        $me['visibility'] = visibility($mth);
        $me['static'] = $mth->isStatic();
        $me['abstract'] = $mth->isAbstract();
        $me['final'] = $mth->isFinal();
        $methods[] = (object) $me;
    }

    $e = [
        'name' => $c->getName(),
        'extension' => $c->getExtensionName() ?: 'Core',
        'kind' => $kind,
        'abstract' => $kind === 'class' && $c->isAbstract(),
        'final' => $c->isFinal(),
        'readonly' => $c->isReadOnly(),
        'parent' => $c->getParentClass() !== false ? $c->getParentClass()->getName() : null,
        'interfaces' => $direct,
        'all_interfaces' => $all,
        'constants' => $consts,
        'properties' => $props,
        'methods' => $methods,
    ];
    if ($kind === 'enum') {
        $re = new ReflectionEnum($name);
        $cases = [];
        foreach ($re->getCases() as $case) {
            $cases[] = (object) [
                'name' => $case->getName(),
                'value' => $case instanceof ReflectionEnumBackedCase ? val($case->getBackingValue()) : null,
            ];
        }
        $e['enum'] = (object) [
            'backing_type' => $re->isBacked() ? (string) $re->getBackingType() : null,
            'cases' => $cases,
        ];
    }
    $a = attrs($c);
    if ($a !== []) {
        $e['attributes'] = $a;
    }
    $classes[$c->getName()] = (object) $e;
}

// ---------------------------------------------------------------------------
// constants

$constants = [];
foreach (get_defined_constants(true) as $category => $list) {
    $cat = [];
    foreach ($list as $cname => $cv) {
        $ce = ['value' => val($cv), 'type' => typeName($cv)];
        try {
            $rc = new ReflectionConstant($cname);
            $ce['deprecated'] = $rc->isDeprecated();
        } catch (ReflectionException) {
            $ce['deprecated'] = false;
        }
        $cat[$cname] = (object) $ce;
    }
    $constants[$category] = m($cat);
}

// ---------------------------------------------------------------------------
// ini + extensions

$iniOwner = [];
$extensions = [];
foreach (get_loaded_extensions() as $ext) {
    $re = new ReflectionExtension($ext);
    $entries = $re->getINIEntries();
    foreach ($entries as $k => $_) {
        $iniOwner[$k] = $ext;
    }
    $extensions[$ext] = (object) [
        'version' => $re->getVersion(),
        'dependencies' => m($re->getDependencies()),
        'functions' => count($re->getFunctions()),
        'classes' => count($re->getClassNames()),
        'constants' => count($re->getConstants()),
        'ini' => count($entries),
    ];
}

$ini = [];
foreach (ini_get_all(null, true) as $k => $v) {
    $ini[$k] = (object) [
        'global_value' => val($v['global_value']),
        'local_value' => val($v['local_value']),
        'access' => $v['access'],
        'extension' => $iniOwner[$k] ?? null,
    ];
}

// ---------------------------------------------------------------------------
// write

$flags = JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE | JSON_PRESERVE_ZERO_FRACTION | JSON_THROW_ON_ERROR;
$files = [
    'functions' => m($functions),
    'classes' => m($classes),
    'constants' => m($constants),
    'ini' => m($ini),
    'extensions' => m($extensions),
];
foreach ($files as $file => $data) {
    $json = json_encode($data, $flags) . "\n";
    file_put_contents("$outDir/$file.json", $json);
    printf("%-16s %8d bytes\n", "$file.json", strlen($json));
}
printf("functions=%d classes=%d constants=%d ini=%d extensions=%d\n",
    count($functions), count($classes), array_sum(array_map(fn($c) => count((array) $c), $constants)), count($ini), count($extensions));
