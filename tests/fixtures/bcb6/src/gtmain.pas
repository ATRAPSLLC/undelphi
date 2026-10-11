unit gtmain;

interface

uses
  Classes, Controls, Forms, StdCtrls, ExtCtrls, ComCtrls, gtframe;

type
  TGTMode = (gmIdle, gmBusy, gmDone);
  TGTFlag = (gfOne, gfTwo, gfThree);
  TGTFlags = set of TGTFlag;

  { Non-visual component whose published properties receive one value of
    every TPF0 value type from gtmain.dfm. }
  TGTValues = class(TComponent)
  private
    FBig: Int64;
    FRatio: Single;
    FPrecise: Double;
    FMoney: Currency;
    FWhen: TDateTime;
    FWide: WideString;
    FMode: TGTMode;
    FFlags: TGTFlags;
    FSmall: Integer;
    FMedium: Integer;
    FLarge: Integer;
    FNegative: Integer;
    FEnabled2: Boolean;
    FLink: TComponent;
    FText: string;
    FLongText: string;
    FItems: TStrings;
    procedure SetItems(Value: TStrings);
  public
    constructor Create(AOwner: TComponent); override;
    destructor Destroy; override;
  published
    property Big: Int64 read FBig write FBig;
    property Ratio: Single read FRatio write FRatio;
    property Precise: Double read FPrecise write FPrecise;
    property Money: Currency read FMoney write FMoney;
    property When: TDateTime read FWhen write FWhen;
    property Wide: WideString read FWide write FWide;
    property Mode: TGTMode read FMode write FMode;
    property Flags: TGTFlags read FFlags write FFlags;
    property Small: Integer read FSmall write FSmall;
    property Medium: Integer read FMedium write FMedium;
    property Large: Integer read FLarge write FLarge;
    property Negative: Integer read FNegative write FNegative;
    property Enabled2: Boolean read FEnabled2 write FEnabled2;
    property Link: TComponent read FLink write FLink;
    property Text: string read FText write FText;
    property LongText: string read FLongText write FLongText;
    property Items: TStrings read FItems write SetItems;
  end;

  TGTMainForm = class(TForm)
    Label1: TLabel;
    Memo1: TMemo;
    Image1: TImage;
    Button1: TButton;
    StatusBar1: TStatusBar;
    Frame1: TGTFrame;
    Values1: TGTValues;
    procedure Button1Click(Sender: TObject);
    procedure FormCreate(Sender: TObject);
  end;

var
  GTMainForm: TGTMainForm;

implementation

{$R *.dfm}

{ TGTValues }

constructor TGTValues.Create(AOwner: TComponent);
begin
  inherited Create(AOwner);
  FItems := TStringList.Create;
end;

destructor TGTValues.Destroy;
begin
  FItems.Free;
  inherited Destroy;
end;

procedure TGTValues.SetItems(Value: TStrings);
begin
  FItems.Assign(Value);
end;

{ TGTMainForm }

procedure TGTMainForm.Button1Click(Sender: TObject);
begin
  Close;
end;

procedure TGTMainForm.FormCreate(Sender: TObject);
begin
  Tag := Values1.Small;
end;

end.
