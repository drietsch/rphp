<?php
$a = <<<EOT
foo
	  EOT;
$b = <<<EOT
	  EOT;
$c = <<<'EOT'
foo
 	EOT;
$d = <<<OUT
{$x[<<<IN
in
	 IN]}
    OUT;
$e = <<<OUT
{$x[<<<'IN'
in
	 IN]}
    OUT;
$f = <<<OUT
{$x[<<<IN
 	IN]}
    OUT;
$g = 1;
