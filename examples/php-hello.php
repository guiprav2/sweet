<?php
require_once __DIR__ . '/../php/Sweet.php';

use Sweet\Document;

$document = new Document();
$document->body()->setClass('min-h-screen bg-zinc-100 p-8');
$card = $document->createElement('main')->setClass('bg-white rounded-lg p-6 text-3xl text-zinc-900');
$card->setText('Hello from PHP and sweet-core');
$document->body()->appendChild($card);
$output = $argv[1] ?? __DIR__ . '/php-hello.png';
$document->savePng($output, 800, 480);
fwrite(STDOUT, "Rendered $output\n");
