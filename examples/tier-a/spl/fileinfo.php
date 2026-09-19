<?php
// `SplFileInfo` (ext/spl `spl_directory.c`): the path arithmetic, the two
// kinds of failure (a predicate answers `false`, a metadata accessor
// throws), and the objects it spawns.
//
// Nothing machine-specific is printed: the tree is built under
// `sys_get_temp_dir()` and then entered with `chdir()`, so every path that
// reaches the output is relative and identical on both sides.

// ---- the dump shape ----------------------------------------------------
// The one `var_dump` of an object comes first, so its handle is #1 on both
// sides; everything else about the shape is shown without a handle. php
// synthesizes two private properties, and leaves `fileName` out entirely
// before the constructor has run.
var_dump(new SplFileInfo('sub/d.txt'));
print_r(new SplFileInfo('sub/d.txt'));
echo "\n";
var_dump((new SplFileInfo('sub/d.txt'))->__debugInfo());
$bare = (new ReflectionClass('SplFileInfo'))->newInstanceWithoutConstructor();
print_r($bare);
echo "\n";
var_dump($bare->__debugInfo());

// ---- path arithmetic (no filesystem involved) --------------------------
// php's `_path` is *not* `dirname()`: it scans back to the last slash and
// drops that slash only when more than one character is left, which is why
// `/tmp` has the path `''` and the path info `/`.
$paths = [
    '/a/b/c.txt', '/a/b/c', 'c.txt', '/a/b/', '/a/b//', '/', '//', '.', '..',
    '', 'a/./b', '/a/b/.', 'x.tar.gz', '.hidden', '/a/b/.hidden', '/tmp/',
    'foo/', 'a//b',
];
foreach ($paths as $p) {
    $i = new SplFileInfo($p);
    $parent = $i->getPathInfo();
    printf(
        "%-14s path=%-8s name=%-10s ext=%-7s base=%-10s pathname=%-12s str=%-12s up=%s\n",
        var_export($p, true),
        var_export($i->getPath(), true),
        var_export($i->getFilename(), true),
        var_export($i->getExtension(), true),
        var_export($i->getBasename(), true),
        var_export($i->getPathname(), true),
        var_export((string) $i, true),
        $parent === null ? 'NULL' : var_export((string) $parent, true)
    );
}

// `getBasename()` strips a suffix only when something is left over.
$b = new SplFileInfo('/a/b/a.txt');
var_dump($b->getBasename('.txt'), $b->getBasename('a.txt'), $b->getBasename('zz'), $b->getBasename(''));

// An object whose constructor never ran answers some of these and refuses
// the rest.
$u = $bare;
foreach (['getPathname', 'getPath', '__toString', 'getRealPath', 'getFilename', 'getBasename', 'getExtension', 'getSize', 'isFile'] as $m) {
    try {
        printf("uninit %-12s => %s\n", $m, var_export($u->$m(), true));
    } catch (Throwable $e) {
        printf("uninit %-12s => %s: %s\n", $m, get_class($e), $e->getMessage());
    }
}

// ---- a real tree -------------------------------------------------------
$root = sys_get_temp_dir() . '/rphp-spl-fileinfo';
foreach (['sub/d.txt', 'f.txt', 'link', 'dangling', 'sub', ''] as $leaf) {
    $p = $root . ($leaf === '' ? '' : '/' . $leaf);
    @unlink($p);
    @rmdir($p);
}
@mkdir($root);
@mkdir($root . '/sub');
file_put_contents($root . '/f.txt', "alpha\nbeta\n");
file_put_contents($root . '/sub/d.txt', 'd');
@symlink('f.txt', $root . '/link');
@symlink('nowhere', $root . '/dangling');
chmod($root . '/f.txt', 0644);
chmod($root . '/sub', 0755);
$cwd = getcwd();
chdir($root);

echo "-- metadata\n";
$f = new SplFileInfo('f.txt');
var_dump($f->getSize(), decoct($f->getPerms()), $f->getType());
var_dump($f->isFile(), $f->isDir(), $f->isLink(), $f->isReadable(), $f->isWritable(), $f->isExecutable());
var_dump($f->getRealPath() !== false, basename($f->getRealPath()));

$d = new SplFileInfo('sub');
var_dump(decoct($d->getPerms()), $d->getType(), $d->isFile(), $d->isDir(), $d->isExecutable());

$l = new SplFileInfo('link');
// A symlink follows for `getSize`/`isFile` and does not for
// `getType`/`isLink`/`getLinkTarget`.
var_dump($l->getSize(), $l->getType(), $l->isFile(), $l->isLink(), $l->getLinkTarget());

$dangling = new SplFileInfo('dangling');
var_dump($dangling->getType(), $dangling->isFile(), $dangling->isLink(), $dangling->getRealPath());
var_dump($dangling->getLinkTarget());

echo "-- the accessors that throw\n";
$missing = new SplFileInfo('nope.txt');
foreach (['getPerms', 'getInode', 'getSize', 'getOwner', 'getGroup', 'getATime', 'getMTime', 'getCTime', 'getType', 'getLinkTarget'] as $m) {
    try {
        $missing->$m();
        echo "$m did not throw\n";
    } catch (Throwable $e) {
        printf("%-14s %s: %s\n", $m, get_class($e), $e->getMessage());
    }
}
// …and the ones that quietly answer instead.
var_dump($missing->isFile(), $missing->isDir(), $missing->isLink(), $missing->isReadable(), $missing->getRealPath());

// The class name in the text is a literal, so a subclass sees it too.
class MyInfo extends SplFileInfo
{
}
try {
    (new MyInfo('nope.txt'))->getSize();
} catch (Throwable $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

echo "-- spawning\n";
$f = new SplFileInfo('sub/d.txt');
var_dump(get_class($f->getFileInfo()), (string) $f->getFileInfo());
var_dump(get_class($f->getPathInfo()), (string) $f->getPathInfo());
var_dump(get_class($f->getFileInfo('MyInfo')), get_class($f->getPathInfo('MyInfo')));
$f->setInfoClass('MyInfo');
var_dump(get_class($f->getFileInfo()), get_class($f->getPathInfo()));
foreach (['stdClass', 'ArrayObject', 'NoSuchClassAtAll'] as $c) {
    try {
        $f->getFileInfo($c);
    } catch (Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
    try {
        $f->setInfoClass($c);
    } catch (Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}
try {
    $f->setFileClass('stdClass');
} catch (Throwable $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

echo "-- openFile\n";
$o = (new SplFileInfo('f.txt'))->openFile();
var_dump(get_class($o), $o->fgets(), $o->key());
class MyFile extends SplFileObject
{
}
$withClass = new SplFileInfo('f.txt');
$withClass->setFileClass('MyFile');
var_dump(get_class($withClass->openFile()));
try {
    (new SplFileInfo('nope.txt'))->openFile();
} catch (Throwable $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

echo "-- clone and the deprecated helper\n";
$orig = new SplFileInfo('f.txt');
$copy = clone $orig;
var_dump($orig === $copy, (string) $copy, $copy->getSize());
try {
    $orig->_bad_state_ex();
} catch (Throwable $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

chdir($cwd);
unlink($root . '/sub/d.txt');
unlink($root . '/f.txt');
unlink($root . '/link');
unlink($root . '/dangling');
var_dump(rmdir($root . '/sub'), rmdir($root));
