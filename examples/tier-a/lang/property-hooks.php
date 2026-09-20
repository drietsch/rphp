<?php
// php 8.4 property hooks: a hook body that mentions `$this->name` makes the
// property *backed* (reads and writes without a hook of their own go to the
// slot); a property none of whose hooks does is *virtual* — no slot at all,
// so the missing accessor is an error and nothing dumps it.
class R {
    public string $server { set { echo "set hook\n"; $this->server = $value; } }
    public int $virt { get { return 42; } }
    public string $both { get { return strtoupper($this->both); } set { $this->both = trim($value); } }
    public string $v2 { get => "computed"; }
    public int $wo { set { $this->wo = $value * 2; } }
    public int $v { get { return 1; } set { echo "v set\n"; } }
    public int $b = 3 { set { $this->b = $value + 1; } }
    public string $short = "s" { set => strtolower($value); }
    public function init() { self::setProp($this, 'server', 'raw'); }
    private static function setProp(self $r, string $name, mixed $v): void {
        $p = new ReflectionProperty(self::class, $name);
        $p->setRawValue($r, $v);
    }
}
$r = new R;
$r->init();
var_dump($r->server);
$r->server = 'via-hook';
var_dump($r->server, $r->virt);
$r->both = "  hi  "; var_dump($r->both);
var_dump($r->v2);
$r->wo = 21; var_dump($r->wo);
$r->short = "MiXeD"; var_dump($r->short);
foreach (['server', 'virt', 'both', 'v2', 'wo', 'v', 'b', 'short'] as $n) {
    $p = new ReflectionProperty(R::class, $n);
    echo "$n: virtual=", var_export($p->isVirtual(), true), " hooks=", implode(',', array_keys($p->getHooks())),
        " get=", var_export($p->hasHook(PropertyHookType::Get), true), " set=", var_export($p->hasHook(PropertyHookType::Set), true),
        " init=", var_export($p->isInitialized($r), true), "\n";
}
$p = new ReflectionProperty(R::class, 'both');
var_dump($p->getHook(PropertyHookType::Get)->getName(), $p->getHook(PropertyHookType::Get)->class, $p->getHook(PropertyHookType::Set)->getReturnType()?->getName());
var_dump((new ReflectionProperty(R::class, 'b'))->getHook(PropertyHookType::Get));
echo "--- the raw and hooked accessors ---\n";
foreach (['v', 'virt', 'b'] as $n) {
    $p = new ReflectionProperty(R::class, $n);
    try { $p->setRawValue($r, 5); echo "setRawValue ok\n"; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
    try { var_dump($p->getRawValue($r)); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
    try { var_dump($p->getValue($r)); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
    try { $p->setValue($r, 7); echo "setValue ok\n"; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
}
echo "--- errors ---\n";
try { $r->virt = 1; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { echo $r->wo; echo "\n"; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { unset($r->virt); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { unset($r->b); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
echo "--- the views ---\n";
var_dump(isset($r->virt), isset($r->v), isset($r->b), empty($r->virt), property_exists($r, 'virt'));
var_dump($r);
print_r($r); echo "\n";
var_dump((array) $r, get_object_vars($r), json_encode($r), serialize($r));
$c = clone $r; var_dump($c->both, $c->virt);
var_dump(PropertyHookType::Get, PropertyHookType::Set->value, PropertyHookType::from('get'));
