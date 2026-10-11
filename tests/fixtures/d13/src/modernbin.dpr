program modernbin;

{ A console program around modernrt: the binary whose Delphi 2010+ RTTI,
  attributes, string literals, thread variables and delay-loaded imports
  the binary-side tests (tests/d13.rs) check against this source and the
  linker map. }

{$APPTYPE CONSOLE}

uses
  modernrt in 'modernrt.pas';

begin
  Exercise;
end.
