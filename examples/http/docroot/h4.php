<?php ob_start(); echo "buffered "; header('X-Late: 1'); ob_end_flush(); echo "sent"; header('X-Too-Late: 1'); echo "\n";
