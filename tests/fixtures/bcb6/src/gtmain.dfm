object GTMainForm: TGTMainForm
  Left = 192
  Top = 107
  Width = 640
  Height = 480
  Caption = 'undelphi ground-truth form'
  Color = clBtnFace
  Font.Charset = DEFAULT_CHARSET
  Font.Color = clWindowText
  Font.Height = -11
  Font.Name = 'MS Sans Serif'
  Font.Style = [fsBold, fsItalic]
  KeyPreview = True
  OldCreateOrder = False
  Position = poScreenCenter
  Tag = -5
  OnCreate = FormCreate
  PixelsPerInch = 96
  TextHeight = 13
  object Label1: TLabel
    Left = 8
    Top = 8
    Width = 200
    Height = 13
    Caption = 'Gr'#246#223'e '#955#946#947
  end
  object Memo1: TMemo
    Left = 8
    Top = 32
    Width = 300
    Height = 89
    Lines.Strings = (
      'line one'
      'line two'
      '')
    TabOrder = 0
  end
  object Image1: TImage
    Left = 320
    Top = 32
    Width = 2
    Height = 2
    Picture.Data = {
      07544269746D617046000000424D460000000000000036000000280000000200
      000002000000010018000000000010000000130B0000130B0000000000000000
      00000000FF00FF000000FF0000FFFFFF0000}
  end
  object Button1: TButton
    Left = 8
    Top = 128
    Width = 75
    Height = 25
    Caption = '&Go'
    TabOrder = 1
    OnClick = Button1Click
  end
  object StatusBar1: TStatusBar
    Left = 0
    Top = 434
    Width = 632
    Height = 19
    Panels = <
      item
        Text = 'first panel'
        Width = 120
      end
      item
        Alignment = taCenter
        Text = 'second panel'
        Width = 200
      end>
    SimplePanel = False
  end
  inline Frame1: TGTFrame
    Left = 8
    Top = 160
    Width = 320
    Height = 80
    TabOrder = 2
    inherited Edit1: TEdit
      Text = 'overridden in host'
    end
  end
  object Values1: TGTValues
    Big = 1234567890123
    Ratio = 1.5s
    Precise = 3.25
    Money = 125000c
    When = 37000.5d
    Wide = #1055#1088#1080#1074#1077#1090
    Mode = gmBusy
    Flags = [gfOne, gfThree]
    Small = 100
    Medium = 30000
    Large = 70000
    Negative = -200
    Enabled2 = False
    Link = nil
    Text = ''
    LongText = 
      '0123456789012345678901234567890123456789012345678901234567890123' +
      '0123456789012345678901234567890123456789012345678901234567890123' +
      '0123456789012345678901234567890123456789012345678901234567890123' +
      '0123456789012345678901234567890123456789012345678901234567890123' +
      'tail'
    Items.Strings = (
      'alpha'
      'beta')
    Left = 400
    Top = 300
  end
end
