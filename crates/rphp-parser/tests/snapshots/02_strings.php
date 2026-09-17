<?php
$a = "\101\x41\u{41}\n\t\v\e\f\\\$\"\q\377 $b {$c} {$d[1]} {$e->f()} \{$g}";
$b = 'it\'s \\ \n';
$c = b"x" . B'y';
$d = "$a[0] $a[-1] $a[k] $a[$i] $a->b $a?->b ${n} ${n[1]} {$a::$b} {$a()}";
$e = <<<EOT
    a $a
      b\tc
    EOT;
$f = <<<'NOW'
  raw\n $x
  NOW;
$g = <<<"EOT"
z
EOT . "x";
$h = `ls -la $dir {$x}`;
$i = "";
$j = <<<EOT

EOT;
