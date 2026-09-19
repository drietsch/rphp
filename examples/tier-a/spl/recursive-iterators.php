<?php
// Tier-A differential: SPL's recursive iterator decorators
// (spl_iterators.c) — RecursiveArrayIterator, RecursiveIteratorIterator and
// the recursive filter / regex / caching wrappers.
//
// The whole point is the call *order*: RecursiveIteratorIterator asks
// callHasChildren()/callGetChildren() and runs its begin/end hooks at exact
// moments, and a mode changes which of them report an element. The user
// RecursiveIterator below echoes every call, so the trace is the assertion.

class RLoud implements RecursiveIterator
{
    private array $v;
    private array $k;
    private int $i = 0;

    public function __construct(array $a, public string $n = 'R')
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
        echo "  {$this->n}.current->", is_array($r) ? '[array]' : var_export($r, true), "\n";
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

    public function hasChildren(): bool
    {
        $r = is_array($this->v[$this->i] ?? null);
        echo "  {$this->n}.hasChildren->", var_export($r, true), "\n";
        return $r;
    }

    public function getChildren(): RecursiveIterator
    {
        echo "  {$this->n}.getChildren\n";
        return new RLoud($this->v[$this->i], $this->n . '/');
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

function show(mixed $k, mixed $v): string
{
    return var_export($k, true) . ' => ' . (is_array($v) ? '[array]' : var_export($v, true));
}

$tree = ['a' => 1, 'b' => ['c' => 2, 'd' => 3], 'e' => 4];

echo "== RecursiveArrayIterator ==\n";
$ra = new RecursiveArrayIterator($tree);
var_dump($ra instanceof ArrayIterator, $ra instanceof RecursiveIterator, $ra instanceof SeekableIterator);
foreach ($ra as $k => $v) {
    echo '  ', $k, ': hasChildren=', var_export($ra->hasChildren(), true), "\n";
}
$ra->rewind();
$ra->next();
$kids = $ra->getChildren();
echo '  children class=', get_class($kids), ' flags=', $kids->getFlags(), "\n";
echo '  ', json_encode(iterator_to_array($kids)), "\n";
echo "  -- new static() keeps a subclass\n";
class MyRAI extends RecursiveArrayIterator
{
}
$sub = new MyRAI([[1]]);
$sub->rewind();
echo '  ', get_class($sub->getChildren()), "\n";
echo "  -- an element that already is one is handed straight back\n";
$inner = new RecursiveArrayIterator([5]);
$outer = new RecursiveArrayIterator([$inner]);
$outer->rewind();
var_dump($outer->getChildren() === $inner);
echo "  -- CHILD_ARRAYS_ONLY\n";
$cao = new RecursiveArrayIterator(['o' => new ArrayObject([1])], RecursiveArrayIterator::CHILD_ARRAYS_ONLY);
$cao->rewind();
var_dump($cao->hasChildren(), $cao->getChildren());
echo "  -- a leaf has no children, and asking anyway is the ArrayIterator constructor's error\n";
$leaf = new RecursiveArrayIterator([1]);
$leaf->rewind();
var_dump($leaf->hasChildren());
fail(fn () => $leaf->getChildren());
echo '  CHILD_ARRAYS_ONLY=', RecursiveArrayIterator::CHILD_ARRAYS_ONLY, "\n";

echo "\n== RecursiveIteratorIterator over a RecursiveArrayIterator ==\n";
$deep = ['a' => 1, 'b' => ['c' => 2, 'd' => ['e' => 3]]];
foreach ([0 => 'LEAVES_ONLY', 1 => 'SELF_FIRST', 2 => 'CHILD_FIRST'] as $mode => $name) {
    echo '  -- ', $name, "\n";
    $r = new RecursiveIteratorIterator(new RecursiveArrayIterator($deep), $mode);
    foreach ($r as $k => $v) {
        echo '  ', show($k, $v), ' depth=', $r->getDepth(), "\n";
    }
    echo '  after the walk depth=', $r->getDepth(), "\n";
}

echo "\n== the call pattern, over a user RecursiveIterator ==\n";
class Hooked extends RecursiveIteratorIterator
{
    public function beginIteration(): void
    {
        echo '  #beginIteration d=', $this->getDepth(), "\n";
    }

    public function endIteration(): void
    {
        echo '  #endIteration d=', $this->getDepth(), "\n";
    }

    public function beginChildren(): void
    {
        echo '  #beginChildren d=', $this->getDepth(), "\n";
    }

    public function endChildren(): void
    {
        echo '  #endChildren d=', $this->getDepth(), "\n";
    }

    public function nextElement(): void
    {
        echo '  #nextElement d=', $this->getDepth(), "\n";
    }

    public function callHasChildren(): bool
    {
        $r = parent::callHasChildren();
        echo '  #callHasChildren->', var_export($r, true), "\n";
        return $r;
    }

    public function callGetChildren(): RecursiveIterator
    {
        echo "  #callGetChildren\n";
        return parent::callGetChildren();
    }
}
foreach ([0 => 'LEAVES_ONLY', 1 => 'SELF_FIRST', 2 => 'CHILD_FIRST'] as $mode => $name) {
    echo '  -- ', $name, "\n";
    $r = new Hooked(new RLoud($tree), $mode);
    foreach ($r as $k => $v) {
        echo '  ', show($k, $v), ' depth=', $r->getDepth(),
            ' sub=', get_class($r->getSubIterator()), "\n";
    }
}

echo "\n== depth limits ==\n";
$r = new RecursiveIteratorIterator(new RecursiveArrayIterator($deep));
var_dump($r->getMaxDepth());
$r->setMaxDepth(1);
var_dump($r->getMaxDepth());
foreach ($r as $k => $v) {
    echo '  ', show($k, $v), ' depth=', $r->getDepth(), "\n";
}
$r->setMaxDepth();
var_dump($r->getMaxDepth());
fail(fn () => $r->setMaxDepth(-2));
echo "  -- maxDepth 0 in SELF_FIRST still reports the branch\n";
$r = new RecursiveIteratorIterator(new RecursiveArrayIterator($deep), RecursiveIteratorIterator::SELF_FIRST);
$r->setMaxDepth(0);
foreach ($r as $k => $v) {
    echo '  ', show($k, $v), ' depth=', $r->getDepth(), "\n";
}

echo "\n== getSubIterator / getInnerIterator ==\n";
$r = new RecursiveIteratorIterator(new RecursiveArrayIterator($deep));
$r->rewind();
var_dump($r->getSubIterator(0) === $r->getInnerIterator());
var_dump($r->getSubIterator(5));
var_dump($r->getSubIterator(-1));
echo '  depth before rewind of a fresh one: ';
$fresh = new RecursiveIteratorIterator(new RecursiveArrayIterator($deep));
var_dump($fresh->getDepth());
var_dump($fresh->getSubIterator() === null);
var_dump($fresh->callHasChildren());

echo "\n== construction ==\n";
fail(fn () => new RecursiveIteratorIterator(new ArrayIterator([1])));
fail(fn () => new RecursiveIteratorIterator(new stdClass()));
fail(fn () => new RecursiveIteratorIterator([1]));
fail(fn () => new RecursiveIteratorIterator('x'));
class RAgg implements IteratorAggregate
{
    public function getIterator(): Traversable
    {
        echo "  RAgg.getIterator\n";
        return new RecursiveArrayIterator(['z' => [9]]);
    }
}
foreach (new RecursiveIteratorIterator(new RAgg()) as $k => $v) {
    echo '  ', show($k, $v), "\n";
}
echo '  constants: ', RecursiveIteratorIterator::LEAVES_ONLY, ' ', RecursiveIteratorIterator::SELF_FIRST,
    ' ', RecursiveIteratorIterator::CHILD_FIRST, ' ', RecursiveIteratorIterator::CATCH_GET_CHILD, "\n";

echo "\n== CATCH_GET_CHILD ==\n";
class Boom extends RLoud
{
    public function getChildren(): RecursiveIterator
    {
        echo "  Boom.getChildren throws\n";
        throw new UnexpectedValueException('no children here');
    }
}
$boom = ['a' => 1, 'b' => [2], 'c' => 3];
fail(function () use ($boom) {
    foreach (new RecursiveIteratorIterator(new Boom($boom)) as $k => $v) {
        echo '  ', show($k, $v), "\n";
    }
});
echo "  -- with the flag the branch is skipped\n";
$caught = new RecursiveIteratorIterator(
    new Boom($boom),
    RecursiveIteratorIterator::LEAVES_ONLY,
    RecursiveIteratorIterator::CATCH_GET_CHILD
);
foreach ($caught as $k => $v) {
    echo '  ', show($k, $v), "\n";
}
echo "  -- the flag covers a refusing hasChildren() too, which then reads as a leaf\n";
class BoomHas extends RLoud
{
    public function hasChildren(): bool
    {
        echo "  BoomHas.hasChildren throws\n";
        throw new UnexpectedValueException('cannot tell');
    }
}
fail(function () {
    foreach (new RecursiveIteratorIterator(new BoomHas(['a' => 1])) as $k => $v) {
        echo '  ', show($k, $v), "\n";
    }
});
$hc = new RecursiveIteratorIterator(
    new BoomHas(['a' => 1]),
    RecursiveIteratorIterator::LEAVES_ONLY,
    RecursiveIteratorIterator::CATCH_GET_CHILD
);
foreach ($hc as $k => $v) {
    echo '  ', show($k, $v), "\n";
}

echo "\n== RecursiveFilterIterator ==\n";
class OddRF extends RecursiveFilterIterator
{
    public function accept(): bool
    {
        $v = $this->current();
        $r = is_array($v) || $v % 2 === 1;
        echo '  accept(', is_array($v) ? '[array]' : var_export($v, true), ')->', var_export($r, true), "\n";
        return $r;
    }
}
$f = new OddRF(new RecursiveArrayIterator($tree));
foreach (new RecursiveIteratorIterator($f) as $k => $v) {
    echo '  ', show($k, $v), "\n";
}
echo "  -- getChildren keeps the subclass, wrapping the inner one's children\n";
$f2 = new OddRF(new RecursiveArrayIterator(['x' => ['y' => 1]]));
$f2->rewind();
$kids = $f2->getChildren();
echo '  ', get_class($kids), ' over ', get_class($kids->getInnerIterator()), "\n";
var_dump($f2->hasChildren());
fail(fn () => new RecursiveFilterIterator(new RecursiveArrayIterator([1])));
fail(fn () => new OddRF(new ArrayIterator([1])));

echo "\n== RecursiveCallbackFilterIterator ==\n";
$rc = new RecursiveCallbackFilterIterator(
    new RecursiveArrayIterator($tree),
    function ($v, $k, $inner) {
        $r = is_array($v) || $v % 2 === 1;
        echo '  cb(', is_array($v) ? '[array]' : var_export($v, true), ', ', var_export($k, true),
            ', ', get_class($inner), ')->', var_export($r, true), "\n";
        return $r;
    }
);
foreach (new RecursiveIteratorIterator($rc) as $k => $v) {
    echo '  ', show($k, $v), "\n";
}
$rc2 = new RecursiveCallbackFilterIterator(new RecursiveArrayIterator(['x' => ['y' => 1]]), fn () => true);
$rc2->rewind();
echo '  ', get_class($rc2->getChildren()), "\n";
fail(fn () => new RecursiveCallbackFilterIterator(new ArrayIterator([1]), fn () => true));

echo "\n== ParentIterator ==\n";
$p = new ParentIterator(new RLoud($tree));
foreach (new RecursiveIteratorIterator($p, RecursiveIteratorIterator::SELF_FIRST) as $k => $v) {
    echo '  ', show($k, $v), "\n";
}
$p2 = new ParentIterator(new RecursiveArrayIterator(['x' => ['y' => 1]]));
$p2->rewind();
echo '  ', get_class($p2->getChildren()), "\n";
fail(fn () => new ParentIterator(new ArrayIterator([1])));

echo "\n== RecursiveRegexIterator ==\n";
$rr = new RecursiveRegexIterator(
    new RecursiveArrayIterator(['foo' => 'aaa', 'bar' => ['baz' => 'abc', 'qux' => 'zzz']]),
    '/^a/'
);
foreach (new RecursiveIteratorIterator($rr) as $k => $v) {
    echo '  ', show($k, $v), "\n";
}
$rr->rewind();
var_dump($rr->hasChildren(), $rr->accept());
echo "  -- an empty branch is rejected\n";
$rr2 = new RecursiveRegexIterator(new RecursiveArrayIterator(['x' => [], 'y' => 'abc']), '/^a/');
$rr2->rewind();
var_dump($rr2->valid(), $rr2->key());
echo "  -- getChildren carries the pattern\n";
$rr3 = new RecursiveRegexIterator(new RecursiveArrayIterator(['b' => ['c' => 'abc']]), '/^a/');
$rr3->rewind();
$kid = $rr3->getChildren();
echo '  ', get_class($kid), ' regex=', $kid->getRegex(), ' mode=', $kid->getMode(), "\n";
fail(fn () => new RecursiveRegexIterator(new ArrayIterator([1]), '/x/'));

echo "\n== RecursiveCachingIterator ==\n";
$rci = new RecursiveCachingIterator(new RecursiveArrayIterator(['a' => 1, 'b' => ['c' => 2]]), CachingIterator::TOSTRING_USE_KEY);
$rci->rewind();
var_dump($rci->valid(), $rci->key(), $rci->hasChildren(), $rci->getChildren());
echo '  string="', (string) $rci, "\"\n";
$rci->next();
var_dump($rci->key(), $rci->hasChildren());
echo '  children class=', get_class($rci->getChildren()), "\n";
foreach (new RecursiveIteratorIterator(new RecursiveCachingIterator(new RecursiveArrayIterator(['a' => 1, 'b' => ['c' => 2]]), CachingIterator::TOSTRING_USE_KEY)) as $k => $v) {
    echo '  ', show($k, $v), "\n";
}
echo "  -- CachingIterator::CATCH_GET_CHILD swallows a refusing getChildren()\n";
class BoomGet extends RLoud
{
    public function getChildren(): RecursiveIterator
    {
        echo "  BoomGet.getChildren throws\n";
        throw new UnexpectedValueException('cannot descend');
    }
}
fail(function () {
    $c = new RecursiveCachingIterator(new BoomGet(['a' => [1]]), CachingIterator::TOSTRING_USE_KEY);
    $c->rewind();
});
$cc = new RecursiveCachingIterator(new BoomGet(['a' => [1]]), CachingIterator::CATCH_GET_CHILD);
$cc->rewind();
var_dump($cc->hasChildren(), $cc->key());
fail(fn () => new RecursiveCachingIterator(new ArrayIterator([1])));

echo "\nDONE\n";
