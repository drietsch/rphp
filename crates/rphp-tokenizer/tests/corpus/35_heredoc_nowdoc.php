<?php
$a = <<<'EOT'
no $interp {$here} ${or} here \n
EOT;
$b = <<<'EOT'
  indented
  EOT;
$c = <<<'EOT'
EOT;
$d = <<<'E'
x
E;
$e = <<<'EOT'
EOTX
EOT;
$f = <<<'EOT'
	tab indent
	EOT;
$g = <<<'EOT'
 EOT
EOT;
$h = <<<'EOT'
a
 EOT;
$i = <<<'EOT'
  a
 EOT;
$j = <<<'EOT'
\
EOT;
$k = <<<'EOT'
$a[0] {$b} "x" 'y' `z` ?> <?php
EOT;
$l = <<<'EOT'
EOT
EOT;
$m = <<< 'EOT'
space before quoted label
EOT;
$n = B<<<'EOT'
binary nowdoc
EOT;
