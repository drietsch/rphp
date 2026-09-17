<?php "a ?> b"; 'a ?> b'; $x = <<<EOT
?>
EOT;
$y = `?>`; $z = "$a[0]?>"; "{$a}?>"; $w = <<<'EOT'
?>
EOT;
$v; // x ?>
html
<?php # ?>z
<?php /* ?> */ ?>
<?php $a-> // ?>
<?php $a->?>
