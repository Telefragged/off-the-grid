{ pkgs ? import <nixpkgs> {} }:
let
    compiler-jar = pkgs.fetchurl {
        url = "https://github.com/ergoplatform/ergoscript-compiler/releases/download/v0.1/ErgoScriptCompiler-assembly-0.1.jar";
        sha256 = "1r2bad2q271s0j1mq5yk4c9g13nd7sjwhw9b5fmq2xrw1bdr7xy4";
    };

    jre = pkgs.jre;
    xxd = pkgs.unixtools.xxd;
in pkgs.runCommand "compile-contract" {} ''
    mkdir -p $out
    ${jre}/bin/java -cp ${compiler-jar} Compile ${./contract.es} ${./symbols.json} > $out/result
    head -n2 $out/result | tail -n1 | tr -d '\n'| ${xxd}/bin/xxd -r -p > $out/ergotree
''
