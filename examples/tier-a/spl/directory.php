<?php
// The four directory iterators (ext/spl `spl_directory.c`):
// `DirectoryIterator`, `FilesystemIterator`, `RecursiveDirectoryIterator`
// and `GlobIterator`.
//
// `readdir(3)` order is not stable across filesystems, so every listing is
// collected and `sort()`ed before it is printed. The tree lives under
// `sys_get_temp_dir()` and is entered with `chdir()`, so every path in the
// output is relative.

$root = sys_get_temp_dir() . '/rphp-spl-directory';

/** Remove the tree, deepest first, so a re-run starts clean. */
function scrub(string $dir): void
{
    foreach (@scandir($dir) ?: [] as $e) {
        if ($e === '.' || $e === '..') {
            continue;
        }
        $p = $dir . '/' . $e;
        if (is_dir($p) && !is_link($p)) {
            scrub($p);
        } else {
            @unlink($p);
        }
    }
    @rmdir($dir);
}

scrub($root);
mkdir($root . '/sub/deep', 0777, true);
mkdir($root . '/empty');
file_put_contents($root . '/a.txt', "one\ntwo\n");
file_put_contents($root . '/b.md', 'b');
file_put_contents($root . '/sub/c.txt', 'c');
file_put_contents($root . '/sub/deep/d.txt', 'd');
@symlink('sub', $root . '/link-to-sub');
$cwd = getcwd();
chdir($root);

/** Print a set of strings in a stable order. */
function show(string $label, array $rows): void
{
    sort($rows);
    echo $label, ': ', implode(' | ', $rows), "\n";
}

// ---- DirectoryIterator -------------------------------------------------
// `current()` hands back the iterator itself, so every element of the loop
// is the same object.
$di = new DirectoryIterator('.');
$rows = [];
$same = true;
foreach ($di as $k => $v) {
    $same = $same && $v === $di;
    $rows[] = sprintf(
        '%s(dot=%s,ext=%s,str=%s,key=%s)',
        $v->getFilename(),
        var_export($v->isDot(), true),
        var_export($v->getExtension(), true),
        var_export((string) $v, true),
        gettype($k)
    );
}
show('DirectoryIterator', $rows);
var_dump($same, get_class($di->current()), $di->valid(), $di->key());
var_dump($di->getPath());

// Every element of `iterator_to_array` is the one handle, and `rewind()`
// re-reads the directory.
$all = iterator_to_array(new DirectoryIterator('.'), false);
var_dump(count($all), $all[0] === $all[1]);

// `seek()` counts against php's index, drives `rewind`/`valid`/`next`
// through the object, and refuses to run off the end.
$s = new DirectoryIterator('.');
$s->seek(2);
var_dump($s->key(), $s->valid());
$s->seek(0);
var_dump($s->key());
$s->seek(-5);
var_dump($s->key());
try {
    $s->seek(1000);
} catch (Throwable $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
// Seeking exactly to the count lands one past the end without throwing.
$n = iterator_count(new DirectoryIterator('.'));
$s->rewind();
$s->seek($n);
var_dump($n, $s->valid(), $s->key());

// The index keeps climbing past the end; `current()` still answers.
$past = new DirectoryIterator('.');
while ($past->valid()) {
    $past->next();
}
$past->next();
$past->next();
var_dump($past->key(), $past->valid(), get_class($past->current()), $past->getFilename(), $past->isDot());

// `clone` copies the cursor and then goes its own way.
$c1 = new DirectoryIterator('.');
$c1->next();
$c2 = clone $c1;
var_dump($c1->key() === $c2->key(), $c1->getFilename() === $c2->getFilename());
$c2->next();
var_dump($c1->key(), $c2->key());

// ---- FilesystemIterator ------------------------------------------------
var_dump((new FilesystemIterator('.'))->getFlags());
var_dump((new RecursiveDirectoryIterator('.'))->getFlags());
var_dump((new GlobIterator('*'))->getFlags());

$modes = [
    'CURRENT_AS_FILEINFO' => FilesystemIterator::CURRENT_AS_FILEINFO,
    'CURRENT_AS_PATHNAME' => FilesystemIterator::CURRENT_AS_PATHNAME,
    'CURRENT_AS_SELF' => FilesystemIterator::CURRENT_AS_SELF,
    'KEY_AS_FILENAME' => FilesystemIterator::KEY_AS_FILENAME,
    'NEW_CURRENT_AND_KEY' => FilesystemIterator::NEW_CURRENT_AND_KEY,
    'FOLLOW_SYMLINKS' => FilesystemIterator::FOLLOW_SYMLINKS,
    'UNIX_PATHS' => FilesystemIterator::UNIX_PATHS,
];
foreach ($modes as $name => $flag) {
    $it = new FilesystemIterator('.', $flag | FilesystemIterator::SKIP_DOTS);
    $rows = [];
    foreach ($it as $k => $v) {
        $rows[] = sprintf(
            '%s => %s',
            $k,
            is_object($v) ? get_class($v) . '(' . $v . ')' : var_export($v, true)
        );
    }
    show(sprintf('%-20s flags=%-6d', $name, $it->getFlags()), $rows);
}

// Without `SKIP_DOTS` the dots come back.
$rows = [];
foreach (new FilesystemIterator('.', 0) as $k => $v) {
    $rows[] = $k;
}
show('no SKIP_DOTS', $rows);

// `setFlags()` replaces the three mode masks and drops everything else.
$fi = new FilesystemIterator('.');
foreach ([FilesystemIterator::CURRENT_AS_PATHNAME, 0, 1, 15, 0xFFFF] as $f) {
    $fi->setFlags($f);
    printf("setFlags(%d) => %d\n", $f, $fi->getFlags());
}
try {
    $fi->setFlags('x');
} catch (Throwable $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

// An exhausted `FilesystemIterator` still has a key, and only the
// `SplFileInfo` mode refuses to build one.
$dead = new FilesystemIterator('.', FilesystemIterator::SKIP_DOTS);
while ($dead->valid()) {
    $dead->next();
}
var_dump($dead->key(), $dead->getFilename());
try {
    $dead->current();
} catch (Throwable $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
$dead->setFlags(FilesystemIterator::CURRENT_AS_PATHNAME);
var_dump($dead->current());
$dead->setFlags(FilesystemIterator::CURRENT_AS_SELF);
var_dump(get_class($dead->current()));

// ---- RecursiveDirectoryIterator ----------------------------------------
// Its default flags are `0`, so a plain walk visits `.` and `..` — and
// `hasChildren()` says no to both.
$rows = [];
$rdi = new RecursiveDirectoryIterator('.');
foreach ($rdi as $k => $v) {
    $rows[] = sprintf(
        '%s(%s,children=%s,sub=%s)',
        $k,
        get_class($v),
        var_export($rdi->hasChildren(), true),
        var_export($rdi->getSubPathname(), true)
    );
}
show('RecursiveDirectoryIterator', $rows);
var_dump($rdi->getSubPath());

// A symlinked directory is not descended into unless asked.
$links = new RecursiveDirectoryIterator('.', FilesystemIterator::SKIP_DOTS);
foreach ($links as $k => $v) {
    if ($v->getFilename() === 'link-to-sub') {
        var_dump($links->hasChildren(), $links->hasChildren(true));
    }
}
$follow = new RecursiveDirectoryIterator('.', FilesystemIterator::SKIP_DOTS | FilesystemIterator::FOLLOW_SYMLINKS);
foreach ($follow as $k => $v) {
    if ($v->getFilename() === 'link-to-sub') {
        var_dump($follow->hasChildren());
    }
}

// `getChildren()` keeps the class and the flags and extends the sub-path.
class MyRdi extends RecursiveDirectoryIterator
{
}
$rows = [];
$parent = new MyRdi('.', FilesystemIterator::SKIP_DOTS | FilesystemIterator::KEY_AS_FILENAME);
foreach ($parent as $k => $v) {
    if ($parent->hasChildren()) {
        $child = $parent->getChildren();
        // The child's own `getSubPathname()` names whatever entry it
        // landed on, which is `readdir(3)` order, so only the sub-path and
        // the flags are printed.
        $rows[] = sprintf(
            '%s -> %s flags=%d subPath=%s path=%s count=%d',
            $k,
            get_class($child),
            $child->getFlags(),
            var_export($child->getSubPath(), true),
            var_export($child->getPath(), true),
            iterator_count($child)
        );
    }
}
show('getChildren', $rows);

// Descending into something that is not a directory is the constructor's
// exception, raised from inside `getChildren()`.
$leaf = new RecursiveDirectoryIterator('.', FilesystemIterator::SKIP_DOTS);
foreach ($leaf as $v) {
    if ($v->getFilename() === 'a.txt') {
        try {
            $leaf->getChildren();
        } catch (Throwable $e) {
            echo get_class($e), ': ', $e->getMessage(), "\n";
        }
    }
}

// `var_dump` of a `RecursiveDirectoryIterator` shows the four private
// members php synthesizes.
// `sub/deep` holds one entry, so the cursor lands somewhere definite.
$dump = new RecursiveDirectoryIterator('sub/deep', FilesystemIterator::SKIP_DOTS);
var_dump($dump->__debugInfo());

// ---- RecursiveIteratorIterator over it ---------------------------------
foreach ([
    'LEAVES_ONLY' => RecursiveIteratorIterator::LEAVES_ONLY,
    'SELF_FIRST' => RecursiveIteratorIterator::SELF_FIRST,
    'CHILD_FIRST' => RecursiveIteratorIterator::CHILD_FIRST,
] as $name => $mode) {
    $rii = new RecursiveIteratorIterator(
        new RecursiveDirectoryIterator('.', FilesystemIterator::SKIP_DOTS),
        $mode
    );
    $rows = [];
    foreach ($rii as $k => $v) {
        $rows[] = sprintf(
            '%s(depth=%d,sub=%s)',
            $k,
            $rii->getDepth(),
            var_export($rii->getSubIterator()->getSubPathname(), true)
        );
    }
    show($name, $rows);
}

// Sorting the rows above hides the one thing the three modes disagree
// about, so the relative position of a directory and its children is
// stated on its own — which is order-independent within a directory.
foreach ([
    'LEAVES_ONLY' => RecursiveIteratorIterator::LEAVES_ONLY,
    'SELF_FIRST' => RecursiveIteratorIterator::SELF_FIRST,
    'CHILD_FIRST' => RecursiveIteratorIterator::CHILD_FIRST,
] as $name => $mode) {
    $keys = [];
    foreach (new RecursiveIteratorIterator(new RecursiveDirectoryIterator('.', FilesystemIterator::SKIP_DOTS), $mode) as $k => $v) {
        $keys[] = $k;
    }
    $dir = array_search('./sub', $keys, true);
    $child = array_search('./sub/c.txt', $keys, true);
    printf(
        "%-12s directory listed=%s before-its-child=%s\n",
        $name,
        var_export($dir !== false, true),
        $dir === false ? 'n/a' : var_export($dir < $child, true)
    );
}

// With `KEY_AS_FILENAME` the walk keys on the bare name and the recursion
// still works.
$rii = new RecursiveIteratorIterator(
    new RecursiveDirectoryIterator('.', FilesystemIterator::SKIP_DOTS | FilesystemIterator::KEY_AS_FILENAME)
);
$rows = [];
foreach ($rii as $k => $v) {
    $rows[] = $k . '=>' . ($v->isFile() ? $v->getSize() : $v->getType());
}
show('KEY_AS_FILENAME walk', $rows);

// ---- GlobIterator ------------------------------------------------------
$g = new GlobIterator('*.txt');
var_dump($g instanceof Countable, $g instanceof FilesystemIterator, $g->count());
$rows = [];
foreach ($g as $k => $v) {
    $rows[] = $k . ' => ' . get_class($v) . '(' . $v . ')';
}
show('GlobIterator', $rows);
var_dump($g->getPath());

$g2 = new GlobIterator('sub/*');
$rows = [];
foreach ($g2 as $k => $v) {
    $rows[] = $k . ' path=' . $g2->getPath() . ' name=' . $g2->getFilename();
}
show('GlobIterator sub', $rows);
var_dump($g2->count(), $g2->getPath());

$none = new GlobIterator('*.nothing');
var_dump($none->count(), $none->valid(), $none->key(), iterator_count($none));
$scheme = new GlobIterator('glob://*.txt');
var_dump($scheme->count());
$g3 = new GlobIterator('*.txt', FilesystemIterator::KEY_AS_FILENAME | FilesystemIterator::CURRENT_AS_PATHNAME);
$rows = [];
foreach ($g3 as $k => $v) {
    $rows[] = "$k => $v";
}
show('GlobIterator flags', $rows);
try {
    clone $g3;
} catch (Throwable $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

// ---- the constructors that refuse -------------------------------------
class MyDi extends DirectoryIterator
{
}
class MyFi extends FilesystemIterator
{
}
foreach (['DirectoryIterator', 'FilesystemIterator', 'RecursiveDirectoryIterator', 'MyDi', 'MyFi', 'MyRdi', 'GlobIterator'] as $class) {
    foreach (['nope', 'a.txt', ''] as $arg) {
        try {
            new $class($arg);
            printf("%-26s %-8s ok\n", $class, var_export($arg, true));
        } catch (Throwable $e) {
            printf("%-26s %-8s %s: %s\n", $class, var_export($arg, true), get_class($e), $e->getMessage());
        }
    }
}

chdir($cwd);
scrub($root);
var_dump(is_dir($root));
