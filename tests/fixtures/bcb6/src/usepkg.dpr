{ Executable linked against the gtpkg and rtl runtime packages.

  TGTLocal derives from TGTShared, which lives in gtpkg.bpl: the parent
  reference crosses a module boundary, and PACKAGEINFO lists the required
  packages instead of containing their units.

  Keep this source and tests/programs.rs in lockstep. }
program usepkg;
{$APPTYPE CONSOLE}
uses
  Classes, gtshared;

type
  TGTLocal = class(TGTShared)
  private
    FExtra: Integer;
  published
    property Extra: Integer read FExtra write FExtra;
  end;

var
  L: TGTLocal;
begin
  L := TGTLocal.Create;
  L.Caption := MakeShared.Caption;
  Writeln(L.ClassName, ' ', L.ClassParent.ClassName, ' ', L.Caption);
  L.Free;
end.
