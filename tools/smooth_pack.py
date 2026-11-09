#!/usr/bin/env python3
"""Fabrique dossier/tiles à partir de dossier/originals par un simple agrandissement lissé (test du lot, PAS une vraie amélioration).
Usage : tools/smooth_pack.py DOSSIER [N=3] [bicubic|lanczos|nearest]"""
import os, sys
from PIL import Image
d = sys.argv[1]; n = int(sys.argv[2]) if len(sys.argv) > 2 else 3
mode = {'bicubic': Image.BICUBIC, 'lanczos': Image.LANCZOS, 'nearest': Image.NEAREST}[sys.argv[3] if len(sys.argv) > 3 else 'bicubic']
os.makedirs(f'{d}/tiles', exist_ok=True)
for f in os.listdir(f'{d}/originals'):
    Image.open(f'{d}/originals/{f}').convert('RGB').resize((8 * n, 8 * n), mode).save(f'{d}/tiles/{f}')
print(len(os.listdir(f'{d}/tiles')), 'tuiles écrites')
