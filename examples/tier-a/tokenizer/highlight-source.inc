<?php
namespace App\M;
use Foo\Bar as Baz;
#[Attr(1)]
final class X extends \Base implements I {
    public function __construct(private readonly ?int $n = null) {}
    public function m(): static { $h = <<<EOT
    heredoc $n {$this->n} \n
    EOT;
        $nd = <<<'N'
        raw $x
        N;
        $s = "a\tb\$c ${x} $arr[0] $arr[k] $o->p" . `ls` . 'q\'s';
        return $this?->m() ?? (int) $n + 1_000 + 0x1F + 1e3 + .5;
    }
}
$f = fn($x) => $x <=> 2 ** 3;
goto end; end: echo __LINE__, __FILE__, __DIR__, __NAMESPACE__, __FUNCTION__, __METHOD__, __PROPERTY__ ?? 0;
/* block */ /** doc */ # hash
$x = [1 => 'a', "b" => &$y]; $x[] = 5; unset($x); yield from g(); match(true) { default => 1 };
?>
tail html & <b>"quoted"</b> 'single'
<?php declare(strict_types=1); ?><?= "e" ?>
