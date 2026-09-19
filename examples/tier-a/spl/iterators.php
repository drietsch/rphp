<?php
// Tier-A differential: SPL's non-recursive iterator decorators
// (spl_iterators.c) — IteratorIterator and everything layered on it, plus
// EmptyIterator and MultipleIterator.
//
// Every decorator is driven twice: once over an ArrayIterator, and once over
// a user Iterator that echoes each call it receives. The echo trace is the
// point of the snippet — it pins *which* inner methods a decorator calls and
// how often, which is where the parity risk lives.

// A user Iterator that says what is asked of it. Deterministic: no ids, no
// floats, keys and values printed with var_export.
class Loud implements Iterator
{
    private array $v;
    private array $k;
    private int $i = 0;

    public function __construct(array $a, public string $n = 'L')
    {
        $this->v = array_values($a);
        $this->k = array_keys($a);
    }

    public function rewind(): void
    {
        echo "  {$this->n}.rewind\n";
        $this->i = 0;
    }

    public function valid(): bool
    {
        $r = $this->i < count($this->v);
        echo "  {$this->n}.valid->", var_export($r, true), "\n";
        return $r;
    }

    public function current(): mixed
    {
        $r = $this->v[$this->i] ?? null;
        echo "  {$this->n}.current->", var_export($r, true), "\n";
        return $r;
    }

    public function key(): mixed
    {
        $r = $this->k[$this->i] ?? null;
        echo "  {$this->n}.key->", var_export($r, true), "\n";
        return $r;
    }

    public function next(): void
    {
        echo "  {$this->n}.next\n";
        ++$this->i;
    }
}

// A seekable version, for the branch LimitIterator takes when the inner
// iterator can jump.
class LoudSeek extends Loud implements SeekableIterator
{
    private int $len;

    public function __construct(array $a, string $n = 'S')
    {
        parent::__construct($a, $n);
        $this->len = count($a);
    }

    public function seek(int $offset): void
    {
        echo "  {$this->n}.seek({$offset})\n";
        $this->rewind();
        for ($i = 0; $i < $offset; ++$i) {
            $this->next();
        }
    }
}

class Agg implements IteratorAggregate
{
    public function getIterator(): Iterator
    {
        echo "  Agg.getIterator\n";
        return new Loud(['p' => 1, 'q' => 2], 'AI');
    }
}

function fail(callable $f): void
{
    try {
        $f();
        echo "  (no throw)\n";
    } catch (Throwable $e) {
        echo '  ', get_class($e), ': ', $e->getMessage(), "\n";
    }
}

function walk(iterable $it, string $label = 'got'): void
{
    foreach ($it as $k => $v) {
        echo "  {$label} ", var_export($k, true), ' => ', var_export($v, true), "\n";
    }
}

echo "== the interfaces exist ==\n";
foreach (['OuterIterator', 'RecursiveIterator', 'SeekableIterator'] as $i) {
    echo '  ', $i, ' ', var_export(interface_exists($i), true), "\n";
}
var_dump(is_subclass_of('OuterIterator', 'Iterator'));
var_dump(is_subclass_of('IteratorIterator', 'OuterIterator'));
var_dump(is_subclass_of('RegexIterator', 'FilterIterator'));

echo "\n== IteratorIterator ==\n";
$it = new IteratorIterator(new Loud(['a' => 1, 'b' => 2]));
echo "  -- before rewind\n";
var_dump($it->valid(), $it->current(), $it->key());
echo "  -- rewind\n";
$it->rewind();
var_dump($it->valid(), $it->current(), $it->key());
echo "  -- next\n";
$it->next();
var_dump($it->current(), $it->key());
echo "  -- next past the end\n";
$it->next();
var_dump($it->valid(), $it->current());
echo "  -- foreach\n";
walk(new IteratorIterator(new Loud(['x' => 10, 'y' => 20])));
echo "  -- over an ArrayIterator\n";
walk(new IteratorIterator(new ArrayIterator(['a' => 1, 'b' => 2])));
echo "  -- over an IteratorAggregate\n";
$agg = new IteratorIterator(new Agg());
echo '  inner is ', get_class($agg->getInnerIterator()), "\n";
walk($agg);
echo "  -- construction errors\n";
fail(fn () => new IteratorIterator(new stdClass()));
fail(fn () => new IteratorIterator([1, 2]));
fail(fn () => new IteratorIterator(new Agg(), 'Loud'));
fail(fn () => new IteratorIterator(new Agg(), 'Agg'));

echo "\n== FilterIterator ==\n";
class OddFilter extends FilterIterator
{
    public function accept(): bool
    {
        $v = $this->current();
        echo '  accept(', var_export($v, true), ")\n";
        return $v % 2 === 1;
    }
}
walk(new OddFilter(new Loud([1, 2, 3, 4])));
echo "  -- nothing accepted\n";
walk(new OddFilter(new Loud([2, 4])));
echo "  -- over an ArrayIterator\n";
walk(new OddFilter(new ArrayIterator([1, 2, 3, 4, 5])));
echo "  -- errors\n";
fail(fn () => new FilterIterator(new ArrayIterator([1])));
fail(fn () => new OddFilter(new ArrayObject([1])));

echo "\n== CallbackFilterIterator ==\n";
$cb = new CallbackFilterIterator(
    new Loud(['a' => 1, 'b' => 2, 'c' => 3]),
    function ($v, $k, $inner) {
        echo '  cb(', var_export($v, true), ', ', var_export($k, true), ', ', get_class($inner), ")\n";
        return $v !== 2;
    }
);
walk($cb);
walk(new CallbackFilterIterator(new ArrayIterator([1, 'x', 3]), fn ($v) => is_int($v)));
fail(fn () => new CallbackFilterIterator(new ArrayIterator([1]), 'no_such_function_here'));

echo "\n== LimitIterator ==\n";
echo "  -- offset 2, limit 3 over a plain Iterator\n";
$l = new LimitIterator(new Loud([0, 1, 2, 3, 4, 5]), 2, 3);
foreach ($l as $k => $v) {
    echo '  got ', $k, ' => ', $v, ' pos=', $l->getPosition(), "\n";
}
echo "  -- the same over a SeekableIterator\n";
$l = new LimitIterator(new LoudSeek([0, 1, 2, 3, 4, 5]), 2, 3);
foreach ($l as $k => $v) {
    echo '  got ', $k, ' => ', $v, ' pos=', $l->getPosition(), "\n";
}
echo "  -- offset past the end\n";
$l = new LimitIterator(new Loud([0, 1]), 5, 2);
$l->rewind();
var_dump($l->valid(), $l->getPosition());
echo "  -- seek over an ArrayIterator\n";
$l = new LimitIterator(new ArrayIterator([0, 1, 2, 3, 4, 5]), 1, 3);
$l->rewind();
echo '  pos=', $l->getPosition(), ' cur=', $l->current(), "\n";
$l->seek(3);
echo '  pos=', $l->getPosition(), ' cur=', $l->current(), "\n";
fail(fn () => $l->seek(0));
fail(fn () => $l->seek(4));
echo "  -- a backwards seek rewinds a non-seekable inner iterator\n";
$l = new LimitIterator(new Loud([0, 1, 2, 3, 4, 5]), 1, 4);
$l->rewind();
$l->seek(3);
echo '  pos=', $l->getPosition(), "\n";
$l->seek(2);
echo '  pos=', $l->getPosition(), ' cur=', $l->current(), "\n";
echo "  -- errors\n";
fail(fn () => new LimitIterator(new ArrayIterator([1]), -1));
fail(fn () => new LimitIterator(new ArrayIterator([1]), 0, -2));
fail(fn () => new LimitIterator(new ArrayObject([1])));
fail(function () { (new LimitIterator(new ArrayIterator([1]), 0, 0))->rewind(); });

echo "\n== InfiniteIterator ==\n";
$n = 0;
foreach (new InfiniteIterator(new Loud([1, 2])) as $k => $v) {
    echo '  got ', $k, ' => ', $v, "\n";
    if (++$n >= 5) {
        break;
    }
}
echo "  -- an empty inner iterator stops at once\n";
walk(new InfiniteIterator(new Loud([])));

echo "\n== NoRewindIterator ==\n";
$src = new Loud([1, 2, 3]);
$nr = new NoRewindIterator($src);
echo "  -- first pass, stopped after two\n";
$n = 0;
foreach ($nr as $k => $v) {
    echo '  got ', $k, ' => ', $v, "\n";
    if (++$n >= 2) {
        break;
    }
}
echo "  -- second pass resumes where it stopped\n";
walk($nr);
echo "  -- over an ArrayIterator\n";
$nr = new NoRewindIterator(new ArrayIterator([7, 8]));
var_dump($nr->valid(), $nr->current(), $nr->key());
$nr->rewind();
$nr->next();
var_dump($nr->valid(), $nr->current(), $nr->key());

echo "\n== EmptyIterator ==\n";
$e = new EmptyIterator();
var_dump($e instanceof Iterator, $e->valid());
$e->rewind();
$e->next();
fail(fn () => $e->current());
fail(fn () => $e->key());
walk($e);
print_r(iterator_to_array($e));
echo "\n";

echo "\n== AppendIterator ==\n";
$a = new AppendIterator();
var_dump($a->getIteratorIndex(), $a->valid());
echo '  array iterator is ', get_class($a->getArrayIterator()), "\n";
echo "  -- first append\n";
$a->append(new Loud([1, 2], 'A'));
var_dump($a->getIteratorIndex());
echo "  -- second append while the first is still valid\n";
$a->append(new Loud([3], 'B'));
var_dump($a->getIteratorIndex());
echo "  -- foreach\n";
foreach ($a as $k => $v) {
    echo '  got ', $k, ' => ', $v, ' idx=', var_export($a->getIteratorIndex(), true), "\n";
}
var_dump($a->getIteratorIndex());
echo "  -- empty sub-iterators are skipped\n";
$b = new AppendIterator();
$b->append(new Loud([], 'E1'));
$b->append(new Loud([9], 'E2'));
$b->append(new Loud([], 'E3'));
walk($b);
echo "  -- appending after exhaustion picks the new one up\n";
$c = new AppendIterator();
$c->append(new ArrayIterator([1]));
$c->rewind();
$c->next();
var_dump($c->valid());
$c->append(new ArrayIterator([2]));
var_dump($c->valid(), $c->current(), $c->getIteratorIndex());
echo "  -- over ArrayIterators\n";
$d = new AppendIterator();
$d->append(new ArrayIterator(['a' => 1]));
$d->append(new ArrayIterator(['b' => 2]));
walk($d);
print_r(iterator_to_array($d, false));
echo "\n";
fail(fn () => $d->append(new ArrayObject([1])));

echo "\n== CachingIterator ==\n";
$ci = new CachingIterator(new Loud(['a' => 1, 'b' => 2, 'c' => 3]));
echo '  flags=', $ci->getFlags(), "\n";
echo "  -- rewind reads one ahead\n";
$ci->rewind();
var_dump($ci->valid(), $ci->current(), $ci->key(), $ci->hasNext());
echo '  string="', (string) $ci, "\"\n";
echo "  -- next\n";
$ci->next();
var_dump($ci->current(), $ci->hasNext());
$ci->next();
var_dump($ci->current(), $ci->hasNext());
$ci->next();
var_dump($ci->valid(), $ci->hasNext());
echo "  -- foreach over an ArrayIterator\n";
walk(new CachingIterator(new ArrayIterator(['k' => 'v', 'k2' => 'v2'])));
echo "  -- the string flags\n";
foreach ([1 => 'CALL_TOSTRING', 2 => 'TOSTRING_USE_KEY', 4 => 'TOSTRING_USE_CURRENT'] as $f => $name) {
    $c = new CachingIterator(new ArrayIterator(['k1' => 'v1', 'k2' => 'v2']), $f);
    $c->rewind();
    echo '  ', $name, '="', (string) $c, "\"\n";
}
$c = new CachingIterator(new ArrayIterator([1]), 0);
$c->rewind();
fail(fn () => (string) $c);
echo "  -- FULL_CACHE\n";
$fc = new CachingIterator(new ArrayIterator(['x' => 1, 'y' => 2]), CachingIterator::FULL_CACHE);
foreach ($fc as $k => $v) {
    echo '  got ', $k, ' => ', $v, ' cache=', json_encode($fc->getCache()), "\n";
}
echo '  count=', $fc->count(), "\n";
var_dump(isset($fc['x']), $fc['x'], isset($fc['zz']));
$fc['z'] = 9;
echo '  ', json_encode($fc->getCache()), "\n";
unset($fc['x']);
echo '  ', json_encode($fc->getCache()), "\n";
$fc->rewind();
echo '  after rewind: ', json_encode($fc->getCache()), "\n";
echo "  -- without FULL_CACHE every cache method refuses\n";
$nc = new CachingIterator(new ArrayIterator(['a' => 1]));
fail(fn () => $nc->getCache());
fail(fn () => $nc->count());
fail(fn () => $nc->offsetExists('a'));
fail(fn () => $nc->offsetGet('a'));
fail(fn () => $nc->offsetSet('b', 2));
fail(fn () => $nc->offsetUnset('a'));
echo "  -- flag validation\n";
fail(fn () => new CachingIterator(new ArrayIterator([1]), CachingIterator::CALL_TOSTRING | CachingIterator::TOSTRING_USE_KEY));
fail(fn () => new CachingIterator(new ArrayIterator([1]), -1));
$sf = new CachingIterator(new ArrayIterator([1]));
fail(fn () => $sf->setFlags(0));
fail(fn () => $sf->setFlags(3));
$sf->setFlags(CachingIterator::CALL_TOSTRING | CachingIterator::FULL_CACHE);
echo '  flags=', $sf->getFlags(), "\n";
$ui = new CachingIterator(new ArrayIterator([1]), CachingIterator::TOSTRING_USE_INNER);
fail(fn () => $ui->setFlags(0));
$fcOnly = new CachingIterator(new ArrayIterator([1]), CachingIterator::FULL_CACHE);
$fcOnly->setFlags(0);
echo '  FULL_CACHE can be unset: ', $fcOnly->getFlags(), "\n";
fail(fn () => new CachingIterator(new ArrayObject([1])));
echo '  constants: ', CachingIterator::CALL_TOSTRING, ' ', CachingIterator::CATCH_GET_CHILD,
    ' ', CachingIterator::TOSTRING_USE_KEY, ' ', CachingIterator::TOSTRING_USE_CURRENT,
    ' ', CachingIterator::TOSTRING_USE_INNER, ' ', CachingIterator::FULL_CACHE, "\n";

echo "\n== RegexIterator ==\n";
$ri = new RegexIterator(new ArrayIterator(['foo', 'bar', 'foobar']), '/^foo/');
walk($ri);
echo '  regex=', $ri->getRegex(), ' mode=', $ri->getMode(), ' flags=', $ri->getFlags(),
    ' pregFlags=', $ri->getPregFlags(), "\n";
print_r($ri);
echo "\n  -- over a user iterator\n";
walk(new RegexIterator(new Loud(['a' => 'foo', 'b' => 'bar']), '/^foo/'));
echo "  -- GET_MATCH\n";
foreach (new RegexIterator(new ArrayIterator(['foo1', 'bar', 'foo2']), '/^foo(\d)$/', RegexIterator::GET_MATCH) as $k => $v) {
    echo '  ', $k, ' => ', json_encode($v), "\n";
}
echo "  -- ALL_MATCHES\n";
foreach (new RegexIterator(new ArrayIterator(['a1b2', 'zz']), '/(\w)(\d)/', RegexIterator::ALL_MATCHES) as $k => $v) {
    echo '  ', $k, ' => ', json_encode($v), "\n";
}
echo "  -- SPLIT\n";
foreach (new RegexIterator(new ArrayIterator(['a,b,c', 'zz']), '/,/', RegexIterator::SPLIT) as $k => $v) {
    echo '  ', $k, ' => ', json_encode($v), "\n";
}
echo "  -- REPLACE\n";
$rep = new RegexIterator(new ArrayIterator(['a1', 'b2', 'zz']), '/(\d)/', RegexIterator::REPLACE);
$rep->replacement = '<$1>';
foreach ($rep as $k => $v) {
    echo '  ', $k, ' => ', var_export($v, true), "\n";
}
echo "  -- REPLACE with no replacement set\n";
foreach (new RegexIterator(new ArrayIterator(['a1']), '/(\d)/', RegexIterator::REPLACE) as $k => $v) {
    echo '  ', $k, ' => ', var_export($v, true), "\n";
}
echo "  -- USE_KEY matches the key, and REPLACE writes back over it\n";
walk(new RegexIterator(new ArrayIterator(['foo' => 1, 'bar' => 2]), '/^f/', RegexIterator::MATCH, RegexIterator::USE_KEY));
$uk = new RegexIterator(new ArrayIterator(['a1' => 'X']), '/(\d)/', RegexIterator::REPLACE, RegexIterator::USE_KEY);
$uk->replacement = 'Z';
walk($uk);
$ukg = new RegexIterator(new ArrayIterator(['foo1' => 'X']), '/^foo(\d)$/', RegexIterator::GET_MATCH, RegexIterator::USE_KEY);
foreach ($ukg as $k => $v) {
    echo '  ', var_export($k, true), ' => ', json_encode($v), "\n";
}
echo "  -- INVERT_MATCH\n";
walk(new RegexIterator(new ArrayIterator(['foo', 'bar']), '/^f/', RegexIterator::MATCH, RegexIterator::INVERT_MATCH));
echo "  -- preg flags reach preg_match\n";
foreach (new RegexIterator(new ArrayIterator(['foo1']), '/(?<d>\d)/', RegexIterator::GET_MATCH, 0, PREG_OFFSET_CAPTURE) as $v) {
    echo '  ', json_encode($v), "\n";
}
echo "  -- setters\n";
$rs = new RegexIterator(new ArrayIterator([1]), '/x/');
$rs->setMode(RegexIterator::SPLIT);
$rs->setFlags(RegexIterator::USE_KEY);
$rs->setPregFlags(PREG_OFFSET_CAPTURE);
echo '  ', $rs->getMode(), ' ', $rs->getFlags(), ' ', $rs->getPregFlags(), "\n";
fail(fn () => $rs->setMode(99));
fail(fn () => new RegexIterator(new ArrayIterator([1]), '/x/', 99));
echo '  constants: ', RegexIterator::USE_KEY, ' ', RegexIterator::INVERT_MATCH, ' ',
    RegexIterator::MATCH, ' ', RegexIterator::GET_MATCH, ' ', RegexIterator::ALL_MATCHES,
    ' ', RegexIterator::SPLIT, ' ', RegexIterator::REPLACE, "\n";

echo "\n== MultipleIterator ==\n";
$m = new MultipleIterator();
echo '  flags=', $m->getFlags(), ' count=', $m->countIterators(), "\n";
var_dump($m->valid());
fail(fn () => $m->current());
fail(fn () => $m->key());
$x = new Loud([1, 2, 3], 'M1');
$y = new Loud([10, 20], 'M2');
$m->attachIterator($x);
$m->attachIterator($y);
echo '  count=', $m->countIterators(), "\n";
var_dump($m->containsIterator($x), $m->containsIterator(new Loud([], 'Z')));
echo "  -- MIT_NEED_ALL\n";
foreach ($m as $k => $v) {
    echo '  ', json_encode($k), ' => ', json_encode($v), "\n";
}
echo "  -- MIT_NEED_ANY\n";
$any = new MultipleIterator(MultipleIterator::MIT_NEED_ANY);
$any->attachIterator(new ArrayIterator([1, 2, 3]));
$any->attachIterator(new ArrayIterator([10]));
foreach ($any as $k => $v) {
    echo '  ', json_encode($k), ' => ', json_encode($v), "\n";
}
echo "  -- MIT_KEYS_ASSOC\n";
$assoc = new MultipleIterator(MultipleIterator::MIT_NEED_ALL | MultipleIterator::MIT_KEYS_ASSOC);
$assoc->attachIterator(new ArrayIterator([1, 2]), 'first');
$assoc->attachIterator(new ArrayIterator([10, 20]), 'second');
foreach ($assoc as $k => $v) {
    echo '  ', json_encode($k), ' => ', json_encode($v), "\n";
}
echo "  -- errors\n";
$dup = new MultipleIterator(MultipleIterator::MIT_KEYS_ASSOC);
$dup->attachIterator(new ArrayIterator([1]), 'k');
fail(fn () => $dup->attachIterator(new ArrayIterator([2]), 'k'));
$nul = new MultipleIterator(MultipleIterator::MIT_KEYS_ASSOC);
$nul->attachIterator(new ArrayIterator([1]));
$nul->rewind();
fail(fn () => $nul->key());
$ex = new MultipleIterator();
$ex->attachIterator(new ArrayIterator([1]));
$ex->rewind();
$ex->next();
var_dump($ex->valid());
fail(fn () => $ex->current());
fail(fn () => $ex->key());
fail(fn () => $ex->attachIterator(new ArrayObject([1])));
fail(fn () => $ex->detachIterator(new ArrayObject([1])));
fail(fn () => $ex->containsIterator(new ArrayObject([1])));
echo "  -- detach\n";
$det = new MultipleIterator();
$one = new ArrayIterator([1]);
$det->attachIterator($one);
$det->detachIterator($one);
$det->detachIterator($one);
echo '  count=', $det->countIterators(), "\n";
$re = new MultipleIterator();
$re->attachIterator($one, 'p');
$re->attachIterator($one, 'q');
echo '  reattach keeps one entry: ', $re->countIterators(), "\n";
$re->setFlags(MultipleIterator::MIT_NEED_ANY);
echo '  flags=', $re->getFlags(), "\n";
echo '  constants: ', MultipleIterator::MIT_NEED_ANY, ' ', MultipleIterator::MIT_NEED_ALL,
    ' ', MultipleIterator::MIT_KEYS_NUMERIC, ' ', MultipleIterator::MIT_KEYS_ASSOC, "\n";

echo "\nDONE\n";
