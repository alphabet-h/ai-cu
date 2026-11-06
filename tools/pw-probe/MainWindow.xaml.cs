using System.Windows;
using System.Windows.Automation;

namespace PwProbe;

/// <summary>
/// ふつうの欄、パスワード欄、パスワードの長さの表示を持つ窓 (D-092)。
/// 長さの表示は、パスワード欄の中身を見ずに、文字が入ったかを UIA の名前で確かめるためにある。
/// </summary>
public partial class MainWindow : Window
{
    /// <summary>試し用の固定の初期値 (12 文字)。本物のパスワードではない。</summary>
    private const string InitialSecret = "probe-secret";

    public MainWindow()
    {
        InitializeComponent();
        // PasswordBox.Password は依存プロパティでないので XAML で束縛できない。コードで入れる。
        // 代入で PasswordChanged が走り、長さの表示も 12 になる。ハンドラーの付け方 (XAML) が変わっても
        // 起動時の表示が正しくなるよう、明示的にも更新する。
        Secret.Password = InitialSecret;
        UpdateLength();
        Loaded += (_, _) => Plain.Focus();
    }

    private void Secret_PasswordChanged(object sender, RoutedEventArgs e)
    {
        UpdateLength();
    }

    /// <summary>長さの文字列を、表示と UIA の名前の両方に同じものを入れる。</summary>
    private void UpdateLength()
    {
        string text = $"パスワードの長さ: {Secret.Password.Length}";
        Length.Text = text;
        AutomationProperties.SetName(Length, text);
    }
}
