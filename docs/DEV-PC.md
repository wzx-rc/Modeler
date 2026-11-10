# Développer sur PC (Windows)

## Installation

1. **Rust** : https://rustup.rs. Installe `rustup-init.exe` et accepte la toolchain stable MSVC.
2. **Visual Studio Build Tools 2022**, avec la charge « Développement Desktop en C++ » (MSVC + SDK Windows). C'est nécessaire pour l'édition de liens.
3. **Git**, puis récupère le code. De préférence, clone le dépôt (avec l'historique) :
   ```
   git clone https://github.com/wzx-rc/Modeler forme
   cd forme
   ```
   Sinon, dézippe l'archive fournie.
4. **Claude Code** (optionnel) : ouvre le dossier. `CLAUDE.md` est lu automatiquement.

Pas d'autre dépendance : wgpu choisit DX12 ou Vulkan, et les polices sont embarquées.

## Commandes

```powershell
cargo run --release -p forme-app --bin forme          # l'app (première compilation : quelques minutes)
cargo test --release -p forme-core                    # tests du noyau
cargo run --release -p forme-app --bin forme-shot -- shots         # toutes les captures
cargo run --release -p forme-app --bin forme-shot -- C:\tmp\x user # un scénario
OUT=/tmp/x cargo run --release -p forme-core --example bake_probe -- parlignes   # le Bake sur ses scènes de test (images dans OUT)
```

- Les projets et exports vont dans `Documents\Forme` (`USERPROFILE`). On peut changer ce dossier avec `FORME_DOCS`.
- `bake_probe` écrit dans `OUT` (par défaut `/tmp`) : sur Windows, mets `OUT` vers un dossier existant.
- `IPAD_ZOOM=0.84` (variable d'environnement) : `forme-shot` reproduit la mise en page de l'iPad.
- Sans vrai GPU (VM, CI), `forme-shot` accepte un GPU logiciel (Vulkan lavapipe ou WARP).

## iPad (.ipa)

- Chaque push sur `main` de `wzx-rc/Modeler` lance `.github/workflows/build.yml` :
  - job `ipad` : compilation `aarch64-apple-ios` sur macOS (Apple Silicon **et** Intel, le premier qui termine publie, car les Mac arm64 de GitHub sont souvent saturés) ;
  - `ios/build_ipa.sh` produit `target/ipa/Forme.ipa` non signé ;
  - la release **`ipa-latest`** est mise à jour : https://github.com/wzx-rc/Modeler/releases/tag/ipa-latest
- Job `linux` : tests du noyau. Avec « Run workflow » et l'option captures, il produit aussi les captures sur GPU logiciel.
- Installation : **Sideloadly** (Windows/Mac) ou AltStore, avec un Apple ID gratuit. Sur l'iPad : Réglages > Confidentialité et sécurité > Mode développeur, puis faire confiance au profil (Réglages > Général > VPN et gestion de l'appareil). Avec un compte gratuit, il faut réinstaller tous les 7 jours.
- `ios/Info.plist` : identifiant `com.forme.app`, `UIFileSharingEnabled` (les projets apparaissent dans l'app Fichiers > Forme).

## Conventions

- Code (commentaires, noms) en anglais ; interface et messages de commit en français.
- Avant de montrer un changement visuel : `forme-shot` sur le scénario concerné, puis regarder l'image.
- Toute modification d'une primitive se fait dans les trois évaluateurs : `sdf.wgsl`, `program.rs`, `bake/field.rs`.
- `cargo test --release -p forme-core` doit rester vert. `bake_probe parlignes` : tous les cas 100 % quads, 0 bord ouvert (scène de Romain : 390 quads).
