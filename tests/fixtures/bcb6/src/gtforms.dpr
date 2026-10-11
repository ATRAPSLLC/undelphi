{ Form-resource (TPF0) ground-truth fixture for the `undelphi` crate.

  Three DFMs with known contents:

    gtmain.dfm   TGTMainForm - every TValueType the text-DFM converter can
                 emit (Int8/16/32/64, Extended, Single, Currency, Date,
                 String, LString, WString, Ident, True/False, Nil, Set, List,
                 Collection, Binary), nested children and an inline frame
    gtframe.dfm  TGTFrame - the frame embedded inline (ffInline)
    gtchild.dfm  TGTChildForm - visual form inheritance (ffInherited) with a
                 child-position modifier (ffChildPos)

  gtforms.res carries a VERSIONINFO resource built from gtforms.rc.

  Keep these sources and tests/programs.rs in lockstep. }
program gtforms;

uses
  Forms,
  gtframe in 'gtframe.pas' {GTFrame: TFrame},
  gtmain in 'gtmain.pas' {GTMainForm},
  gtchild in 'gtchild.pas' {GTChildForm};

{$R *.res}

begin
  Application.Initialize;
  Application.CreateForm(TGTMainForm, GTMainForm);
  Application.CreateForm(TGTChildForm, GTChildForm);
  Application.Run;
end.
