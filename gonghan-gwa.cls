%% gonghan.cls
%% 公文函件LaTeX文档类
%% Version: 4.4
%% Date: 2024-12-23

\NeedsTeXFormat{LaTeX2e}
\ProvidesClass{gonghan-gwa}[2024/12/23 v4.4 Official Letter Document Class]

% ========== 类选项定义 ==========
\newif\if@forcenewpage
\newif\if@autocalc
\newif\ifgwa@hasattachments
\newif\ifgwa@duplex
\newif\ifgwa@phonenotice
\newif\ifgwa@plaindocument
\newif\ifgwa@jointmodeone
\newif\ifgwa@jointrecord
\newif\ifgwa@whitepaper
\newif\ifgwa@meetingagenda

% 默认值
\@forcenewpagetrue    % 默认强制新页
\@autocalctrue        % 默认自动计算份数
\gwa@hasattachmentsfalse
\gwa@duplexfalse      % 默认单面打印，页码居中
\gwa@phonenoticefalse % 默认标准函稿，保留函号和底部版记
\gwa@plaindocumentfalse % 默认非普通公文（普通公文无红头、主送、落款和版记）
\gwa@jointmodeonefalse  % 默认落款不按联合发文多单位并列
\gwa@jointrecordfalse   % 默认版记只有一行承办单位/联系人/电话
\gwa@whitepaperfalse  % 默认非白头件（白头件无红头、无版记）
\gwa@meetingagendafalse % 默认非会议议程（会议议程无红头、无落款、无版记）

% 选项处理
\DeclareOption{forcenewpage}{\@forcenewpagetrue}
\DeclareOption{noforcenewpage}{\@forcenewpagefalse}
\DeclareOption{autocalc}{\@autocalctrue}
\DeclareOption{noautocalc}{\@autocalcfalse}
\DeclareOption{duplex}{\gwa@duplextrue\PassOptionsToClass{twoside}{article}}
\DeclareOption{phonenotice}{\gwa@phonenoticetrue}
\DeclareOption{plaindocument}{\gwa@plaindocumenttrue}
% 联合发文的落款与版记分开控制：发文单位只剩一个时落款回落右侧单列，
% 但版记仍要按“承办单位—联系人—电话”逐行列出，故 jointrecord 可单独开启。
\DeclareOption{jointmodeone}{\gwa@jointmodeonetrue\gwa@jointrecordtrue}
\DeclareOption{jointrecord}{\gwa@jointrecordtrue}
\DeclareOption{whitepaper}{\gwa@whitepapertrue}
\DeclareOption{meetingagenda}{\gwa@meetingagendatrue}

% 传递未知选项给article类
\DeclareOption*{\PassOptionsToClass{\CurrentOption}{article}}

% 处理选项
\ProcessOptions\relax

% 基于article类
\LoadClass{article}

% 加载必需宏包
% 字体由本类统一配置；禁用 ctex 按操作系统自动选择 Windows/Fandol 字体集，
% 这样内置 Tectonic 在无系统字体环境中也不会提前加载额外字体。
\RequirePackage[fontset=none]{ctex}
\RequirePackage{geometry}
\RequirePackage{calc}
\RequirePackage{fancyhdr}
\RequirePackage{enumitem}
\RequirePackage{graphicx}
\RequirePackage{xcolor}
\RequirePackage{booktabs}
\RequirePackage{array}
\RequirePackage{pifont}
\RequirePackage{refcount}
\RequirePackage{ifthen}
\RequirePackage{ragged2e}
\RequirePackage{xstring}  % 用于字符串处理和计数
\RequirePackage{etoolbox}
\RequirePackage{longtable}
\RequirePackage{tabularray} % longtblr 智能表格（自动列宽、表头续页重复）
\RequirePackage{tabularx}
\RequirePackage{pdflscape} % 仅对横向过密的附件逐个启用 PDF 横页

% ========== 字体设置 ==========
% 正文 Markdown 加粗沿用当前字体，通过 xeCJK 自动伪粗实现，不切换为独立黑体。
% 应用内置 Tectonic 编译时会在 \documentclass 前注入 \GwaFontPath，直接按文件
% 加载随应用分发的字体；单独使用导出的 TeX 时则保留系统字体兼容路径。
% 在设置里改用本机字体时，应用会另外注入 \GwaFontSetupHook 顶替下面这段默认设置，
% 钩子内部同样按 \GwaFontPath 分按文件、按名字两种加载方式。
\providecommand{\GwaFontPath}{}
\providecommand{\GwaFontSetupHook}{}
\ifx\GwaFontSetupHook\@empty
\ifx\GwaFontPath\@empty
    \setCJKmainfont[ItalicFont={KaiTi_GB2312}, AutoFakeBold=true]{FangSong_GB2312}
    \IfFontExistsTF{FZXiaoBiaoSong-B05}{%
        \setCJKfamilyfont{xbs}{FZXiaoBiaoSong-B05}%
        \newfontfamily\enbt{FZXiaoBiaoSong-B05}%
    }{%
        \IfFontExistsTF{FZXiaoBiaoSong-B05S}{%
            \setCJKfamilyfont{xbs}{FZXiaoBiaoSong-B05S}%
            \newfontfamily\enbt{FZXiaoBiaoSong-B05S}%
        }{%
            \ClassError{gonghan-gwa}{未找到方正小标宋字体}{请安装 FZXiaoBiaoSong-B05 或 FZXiaoBiaoSong-B05S}%
        }%
    }%
    \setCJKfamilyfont{kaiti}[AutoFakeBold=true]{KaiTi_GB2312}
    \newfontfamily\enkai{KaiTi_GB2312}
    \setCJKfamilyfont{songti}{SimSun}
    \newfontfamily\ennumber{SimSun}
    \newfontfamily\ensong{SimSun}
    \newfontfamily\enheiti{SimHei}
    \setCJKfamilyfont{heiti}{SimHei}
    \setCJKfamilyfont{fangsong}[AutoFakeBold=true]{FangSong_GB2312}
    \setmainfont[ItalicFont={KaiTi_GB2312}]{FangSong_GB2312}
    \setCJKmonofont{SimHei}
    \setmonofont{SimHei}
\else
    \setCJKmainfont[Path={\GwaFontPath}, ItalicFont={KaiTi.ttf}, AutoFakeBold=true]{FangSong.ttf}
    \setCJKfamilyfont{xbs}[Path={\GwaFontPath}]{XiaoBiaoSong.ttf}
    \newfontfamily\enbt[Path={\GwaFontPath}]{XiaoBiaoSong.ttf}
    \setCJKfamilyfont{kaiti}[Path={\GwaFontPath}, AutoFakeBold=true]{KaiTi.ttf}
    \newfontfamily\enkai[Path={\GwaFontPath}]{KaiTi.ttf}
    \setCJKfamilyfont{songti}[Path={\GwaFontPath}]{SimSun.ttf}
    \newfontfamily\ennumber[Path={\GwaFontPath}]{SimSun.ttf}
    \newfontfamily\ensong[Path={\GwaFontPath}]{SimSun.ttf}
    \newfontfamily\enheiti[Path={\GwaFontPath}]{SimHei.ttf}
    \setCJKfamilyfont{heiti}[Path={\GwaFontPath}]{SimHei.ttf}
    \setCJKfamilyfont{fangsong}[Path={\GwaFontPath}, AutoFakeBold=true]{FangSong.ttf}
    \setmainfont[Path={\GwaFontPath}, ItalicFont={KaiTi.ttf}]{FangSong.ttf}
    \setCJKmonofont[Path={\GwaFontPath}]{SimHei.ttf}
    \setmonofont[Path={\GwaFontPath}]{SimHei.ttf}
\fi
\else
    \GwaFontSetupHook
\fi
\newcommand{\bs}{\CJKfamily{xbs}}
\newcommand{\kai}{\CJKfamily{kaiti}}

\newcommand{\song}{\CJKfamily{songti}}
\newcommand{\fs}{\CJKfamily{fangsong}}
\providecommand{\heiti}{\CJKfamily{heiti}}

% 中英文间距调整
\xeCJKsetup{
	CJKglue={\hskip -1pt plus 0.08\baselineskip},
	CJKecglue={\hskip 2pt plus 0.08\baselineskip}
}

% ========== 页面设置 ==========
\geometry{
    a4paper,
    left=28mm plus 1mm minus 1mm,
    height=225mm,
    top=37mm plus 1mm minus 1mm,
    width=156mm,
}

% ========== 页眉页脚设置 ==========
\pagestyle{fancy}
\fancyhf{}
\renewcommand{\headrulewidth}{0pt}
\renewcommand{\footrulewidth}{0pt}

% 页码：单面居中；双面按装订外侧排布（奇数页右、偶数页左）
\ifgwa@duplex
    \fancyfoot[RO]{\zihao{4}\ensong —~\thepage~—}
    \fancyfoot[LE]{\zihao{4}\ensong —~\thepage~—}
\else
    \fancyfoot[C]{\zihao{4}\ensong —~\thepage~—}
\fi

% 第一页样式：无页码
\fancypagestyle{firstpage}{
    \fancyhf{}
    \renewcommand{\headrulewidth}{0pt}
    \renewcommand{\footrulewidth}{0pt}
}

% ========== 行距与垂直节奏设置 ==========
% 公文正文的准确基线距离为 28.98 TeX pt，直接使用 pt，避免单位换算造成偏差。
\newlength{\BodyBaselineSkip}
\setlength{\BodyBaselineSkip}{28.98pt}
\newcommand{\gwa@bodyspacing}{%
    \renewcommand{\baselinestretch}{1.0}%
    \setlength{\baselineskip}{\BodyBaselineSkip}%
    \setlength{\parskip}{0pt}%
}
\gwa@bodyspacing
% 页面内容不足时把余量留在版心底部。禁止 flushbottom 拉伸段间、标题、表格或图片
% 周围的胶量来凑满页面，否则视觉上会误以为正文行距被放大。
\raggedbottom
% 图像/浮动体周围只使用固定间距，不允许页面构造器伸缩这些间距。
\setlength{\floatsep}{\BodyBaselineSkip}
\setlength{\textfloatsep}{\BodyBaselineSkip}
\setlength{\intextsep}{\BodyBaselineSkip}
\setlength{\abovecaptionskip}{0pt}
\setlength{\belowcaptionskip}{0pt}

% ========== 智能表格（与 mdx official 环境一致）==========
\setlength{\LTpre}{6pt}
\setlength{\LTpost}{6pt}
% 表格按四号字、21 磅固定行距排版；LTpre/LTpost 也是不可伸缩的固定距离。
\AtBeginEnvironment{longtblr}{%
    \fontsize{14bp}{21bp}\selectfont\fs
    \setlength{\baselineskip}{21bp}%
    \setlength{\parskip}{0pt}%
}
\SetTblrInner[longtblr]{
    cells={font=\fs, halign=c, valign=m},
    row{1}={font=\heiti\enheiti, bg=white}
}
\DefTblrTemplate{contfoot-text}{default}{下一页继续}
% 公文表格直接列出，不设置“表1/表2”等标题和跨页续表标题。
\SetTblrTemplate{caption}{empty}
\SetTblrTemplate{capcont}{empty}

% ========== 用户接口命令 ==========

% 抬头信息（带默认值）
\newcommand{\IssuingUnit}{XXXX局}           % 发文单位
\newcommand{\SerialNumber}{01}                  % 顺序号
\newcommand{\Year}{\the\year}                   % 年份（自动获取当前年份）
\newcommand{\DepartmentCode}{XX函}              % 发文机关代字
\newcommand{\DocumentNumber}{\quad }                % 发文序号

% 密级信息（默认为空，表示无密级）
\newcommand{\SecurityLevel}{}
\newcommand{\SecurityPeriod}{}
% 指人专办：勾选后由生成器注入“指人专办”，在密级后空一个全角空格并以黑体标注。
\newcommand{\SpecialHandling}{}
% 密级行主体：密级★保密期限 + 可选“指人专办”（前加一个全角空格）。
% 三个抬头（函稿/白头件/会议议程）共用，保证格式一致。
\newcommand{\SecurityLine}{%
    \SecurityLevel{}★\SecurityPeriod{}%
    \ifthenelse{\equal{\SpecialHandling}{}}{}{\hspace{1em}\SpecialHandling}%
}

% 标题和主送（必须由用户设置）
\newcommand{\DocumentTitle}{}
% 标题内容（含字号与换行），由生成器按标题字数与 jieba 分词排布后写入；
% 为空时 \makeletter 回退为默认的 \zihao{2} 单行标题。
\newcommand{\TitleContent}{}
\newcommand{\Recipient}{}

% 正文内容（必须由用户设置）
\newcommand{\MainContent}{}

% 附件内容（可选）。使用公共接口设置后，类会在落款后自动另页排附件。
\newcommand{\AttachmentContent}{}
\newcommand{\SetAttachmentContent}[1]{%
    \renewcommand{\AttachmentContent}{#1}%
    \gwa@hasattachmentstrue
}

% 联合发文模式1由导出器注入：每行两个落款单位，以及多行承办/联系人/电话版记。
\newcommand{\JointSignatureContent}{}
\newcommand{\JointFooterRecord}{}
\newcommand{\SetJointSignatureContent}[1]{\renewcommand{\JointSignatureContent}{#1}}
\newcommand{\SetJointFooterRecord}[1]{\renewcommand{\JointFooterRecord}{#1}}
\newlength{\FooterRecordWidth}

% 落款信息
\newcommand{\SignatureUnit}{\IssuingUnit}        % 落款单位
% 代章：勾选后由生成器注入“（代章）”，直接跟在落款单位后面，不另起一行。
\newcommand{\SignatureSealOnBehalf}{}
\newcommand{\SignatureYear}{\the\year}         % 发文年（自动获取）
\newcommand{\SignatureMonth}{\the\month}       % 发文月（自动获取）
\newcommand{\SignatureDay}{\quad}              % 发文日（空格待填）

% 版记信息
\newcommand{\CopiesTo}{}                        % 抄送单位（默认无）
\newcommand{\ResponsibleUnit}{}                % 承办单位（必须设置）
\newcommand{\ContactPerson}{}                  % 联系人（必须设置）
\newcommand{\ContactPhone}{}                   % 联系电话（必须设置）
\newcommand{\PrintCopies}{}                    % 印刷份数（留空则自动计算）

% ========== 自动计算印刷份数功能 ==========

% 计数器
\newcounter{RecipientCount}
\newcounter{CopiesToCount}
\newcounter{ResponsibleUnitCount}
\newcounter{TotalCopies}

% 计算字符串中单位的数量（通过统计逗号和顿号）
\newcommand{\CountUnits}[2]{%
    \ifthenelse{\equal{#1}{}}{%
        \setcounter{#2}{0}%
    }{%
        \setcounter{#2}{1}%
        \StrCount{#1}{，}[\commacount]%
        \addtocounter{#2}{\commacount}%
        \StrCount{#1}{、}[\caesuracount]%
        \addtocounter{#2}{\caesuracount}%
    }%
}

% 计算总印刷份数
\newcommand{\CalculateTotalCopies}{%
    \CountUnits{\Recipient}{RecipientCount}%
    \CountUnits{\CopiesTo}{CopiesToCount}%
    \CountUnits{\ResponsibleUnit}{ResponsibleUnitCount}%
    \setcounter{TotalCopies}{\value{RecipientCount}}%
    \addtocounter{TotalCopies}{\value{CopiesToCount}}%
    \addtocounter{TotalCopies}{\value{ResponsibleUnitCount}}%
}

% 获取最终的印刷份数
\newcommand{\GetPrintCopies}{%
    \if@autocalc
        \CalculateTotalCopies%
        \arabic{TotalCopies}%
    \else
        \PrintCopies%
    \fi
}

% 将印数固定在抄送段最后一行的版心右端；若当前行放不下，则自动换到下一行右端。
\newcommand{\PrintCopiesAtLineEnd}{%
    \unskip\nobreak\hfill\penalty50\hskip2em\hbox{}\nobreak\hfill
    \mbox{（共印\GetPrintCopies{}份）}%
    \parfillskip=0pt\finalhyphendemerits=0
}

% 抄送使用三字符悬挂缩进，续行与首个抄送单位对齐，不落到“抄送：”下方。
\newcommand{\FooterCopiesLine}{%
    \ifthenelse{\equal{\CopiesTo}{}}{%
        \PrintCopiesAtLineEnd{}%
    }{%
        \hangindent=3em\hangafter=1\relax
        \makebox[3em][l]{抄送：}\CopiesTo{}\PrintCopiesAtLineEnd{}%
    }%
}

% ========== 发文机关标志（红头）==========

% 红头的目标宽度，与红色反线等宽。
\newlength{\HeaderWidth}
\setlength{\HeaderWidth}{15.6cm}
% 字数少时允许拉开的最大字距。达到这个字距仍排不满版心时，
% 整块居中，不再继续散开。
\newcommand{\HeaderMaxGap}{1em}
% 红头字号。
\newcommand{\HeaderFontSize}{29}

\newlength{\gwa@hdnatural}
\newlength{\gwa@hdspread}
\newlength{\gwa@hdblock}

% 红头排版分两种情形：
% 1. 版心排得下——保持字形不变，只在字与字之间均匀加空，避免把字拉变形；
% 2. 版心排不下——沿用整体缩放，保证一行放得下。
\newcommand{\IssuingUnitHeader}{%
    \begingroup
        \bs\enbt\fontsize{\HeaderFontSize}{\HeaderFontSize}\selectfont
        % 默认字距下的自然宽度
        \settowidth{\gwa@hdnatural}{\IssuingUnit{}}%
        % 字距拉到上限时的宽度；与自然宽度之差正好是“字数-1”个最大字距，
        % 这样无需数汉字个数也能判断还能不能撑满版心。
        \begingroup
            \xeCJKsetup{CJKglue={\hskip\HeaderMaxGap}}%
            \settowidth{\@tempdima}{\IssuingUnit{}}%
            \global\gwa@hdspread=\@tempdima
        \endgroup
        \ifdim\gwa@hdnatural<\HeaderWidth
            \ifdim\gwa@hdspread<\HeaderWidth
                \setlength{\gwa@hdblock}{\gwa@hdspread}%
            \else
                \setlength{\gwa@hdblock}{\HeaderWidth}%
            \fi
            \makebox[\HeaderWidth][c]{%
                \makebox[\gwa@hdblock][s]{%
                    \xeCJKsetup{CJKglue={\hskip 0pt plus 1fil}}%
                    \IssuingUnit{}%
                }%
            }%
        \else
            \resizebox{\HeaderWidth}{\HeaderFontSize pt}{\IssuingUnit{}}%
        \fi
    \endgroup
}

% ========== 内部格式命令 ==========

% 抬头命令
\newcommand{\DocumentHeader}{%
    {\centering \color{red}
        \vspace*{-10mm}
        \IssuingUnitHeader \\
        \vspace{-24pt}
        \vspace{4mm}
        \rule{\HeaderWidth}{0.53mm}\\
    }
    \ifgwa@phonenotice
    \else
        \noindent {\heiti \zihao{3} \ttfamily \SerialNumber{}} \hfill {\zihao{3}\DepartmentCode{}〔\Year{}〕\DocumentNumber{}~号}
    \fi

    \ifthenelse{\equal{\SecurityLevel}{}}{}{%
        {\noindent \heiti\enheiti\bfseries \zihao{3} \SecurityLine{} \hfill}
    }
}

% 落款命令
\newcommand{\SignatureContent}{%
    \ifgwa@jointmodeone
        \JointSignatureContent
    \else
    \begin{flushright}
    \begin{minipage}[t]{11cm}
        \begin{center}
        \gwa@bodyspacing
        \SignatureUnit{}\SignatureSealOnBehalf{} \\
        \SignatureYear{}年\SignatureMonth{}月\SignatureDay{}日
        \end{center}
    \end{minipage}
    \end{flushright}
    \fi
}

% ========== 白头件（呈批件）抬头与落款 ==========

% 白头件（呈批件）抬头：顶格密级，密级后空 10 行；无红头、无函号。
\newcommand{\WhitePaperHeader}{%
    \ifthenelse{\equal{\SecurityLevel}{}}{}{%
        {\noindent \heiti\enheiti\bfseries \zihao{3} \SecurityLine{}\par}%
    }%
    \vspace{10\baselineskip}%
}

% 会议议程抬头：顶格密级，密级后空 1 行直接标题；无红头、无函号、无落款。
\newcommand{\MeetingAgendaHeader}{%
    \ifthenelse{\equal{\SecurityLevel}{}}{}{%
        {\noindent \heiti\enheiti\bfseries \zihao{3} \SecurityLine{}\par}%
    }%
    \vspace{\baselineskip}%
}

% 普通公文抬头：只保留可选密级行；无红头、函号及其他修饰。
\newcommand{\PlainDocumentHeader}{%
    \ifthenelse{\equal{\SecurityLevel}{}}{}{%
        {\noindent \heiti\enheiti\bfseries \zihao{3} \SecurityLine{}\par}%
        \vspace{\baselineskip}%
    }%
}

% 白头件落款右侧预留的签字空间宽度。
\newlength{\WhitePaperSignatureRoom}
\setlength{\WhitePaperSignatureRoom}{4cm}

% 白头件落款：落款单位靠左排，右侧留出签字空间；签字一般位于单位右侧。
% 单位与日期之间空 1 行。落款前的“此页无正文”判定由 \WhitePaperClosing 负责。
\newcommand{\WhitePaperSignature}{%
    \noindent\hspace*{\fill}%
    \begin{minipage}[t]{11cm}%
        \raggedleft
        \gwa@bodyspacing
        \SignatureUnit{}\par
        \vspace{\baselineskip}%
        \SignatureYear{}年\SignatureMonth{}月\SignatureDay{}日
    \end{minipage}%
    \hspace*{\WhitePaperSignatureRoom}\par
}

% 版记命令
\newcommand{\FooterRecord}{%
    \ifgwa@jointrecord
        \JointFooterRecord
    \else
    \par
    % 横页环境结束后会新开一张竖页；页首的普通 \vfill 会被 TeX 丢弃，
    % 必须使用带星号的不可丢弃留白，才能保证版记仍落在版心底部。
    \vspace*{\fill}
    \setlength{\parindent}{0pt}
    \arrayrulewidth=1pt
    \setlength{\FooterRecordWidth}{\linewidth}
    \renewcommand{\arraystretch}{1}
    \setlength{\tabcolsep}{0pt}
    \zihao{4}
    \noindent%
    \begin{tabularx}{\FooterRecordWidth}{@{}>{\raggedright\arraybackslash}X>{\centering\arraybackslash}X>{\raggedleft\arraybackslash}p{11em}@{}}
    \toprule[0.6mm]
    \multicolumn{3}{@{}p{\FooterRecordWidth}@{}}{\FooterCopiesLine{}} \\ \midrule[0.3mm]
    承办单位：\ResponsibleUnit{} &
    联系人：\ContactPerson{} &
    联系电话：\ContactPhone{}\\
    \bottomrule[0.6mm]
    \end{tabularx}\par
    \fi
}

% ========== 落款分页与“此页无正文”自动识别 ==========

% 落款与正文之间的间距；正文末页放不下落款时才另起一页并标注“此页无正文”。
\newlength{\ClosingGap}
\setlength{\ClosingGap}{2cm}
% 落款与版记之间至少留出的间距。版记本身用 \vspace*{\fill} 顶到版心底部，
% 页面宽裕时这段留白会自动变大，这里只规定最紧凑时的下限。
\newlength{\SignatureRecordGap}
\setlength{\SignatureRecordGap}{1cm}
% 量高之后再留一点余量，纯粹用于抵消取整误差。
\newlength{\ClosingMeasureSlack}
\setlength{\ClosingMeasureSlack}{2pt}
\newsavebox{\gwa@closingbox}
\newsavebox{\gwa@recordbox}
\newlength{\gwa@closingneed}

\newcommand{\NoBodyNotice}{%
    \vspace*{58pt}%
    {\noindent\hspace*{2em}\zihao{3}（此页无正文）\par}%
    \vspace{5cm}%
}

% 落款定位的公共实现：先把“间距 + 落款”整体装箱量出真实高度，再决定是否另起一页。
% 不用固定估值，是因为估值两头都会错——落款单位换行、联合发文单位增多时落款不止一行，
% 估小了落款会被挤到下一页却不加“此页无正文”，估大了则明明放得下也白白换页。
% #1 为落款内容命令；调用前把版记等额外占位写进 \gwa@closingneed。
\newcommand{\gwa@placeclosing}[1]{%
    \par
    % 量高时不改字体与行距，保持与正文末尾同一环境，箱内布局即最终布局。
    \setbox\gwa@closingbox=\vbox{\vspace{\ClosingGap}#1}%
    \addtolength{\gwa@closingneed}{%
        \dimexpr\ht\gwa@closingbox+\dp\gwa@closingbox+\ClosingMeasureSlack\relax}%
    \if@forcenewpage
        \newpage
        \NoBodyNotice
        #1%
    \else
        \ifdim\dimexpr\pagegoal-\pagetotal\relax<\gwa@closingneed
            \newpage
            \NoBodyNotice
            #1%
        \else
            % 用 \unvbox 原样放回，落款的位置与行距与量高时完全一致。
            \unvbox\gwa@closingbox
        \fi
    \fi
}

% 白头件落款：无版记，只需容纳落款自身。
\newcommand{\WhitePaperClosing}{%
    \setlength{\gwa@closingneed}{0pt}%
    \gwa@placeclosing{\WhitePaperSignature}%
}

% 函稿落款（含联合发文、电话通知）：
% 无附件时版记与落款同页，要把版记高度和两者之间的留白一并算进去；
% 有附件时版记排在附件末尾，正文末页只需容纳落款；电话通知没有版记。
\newcommand{\LetterClosing}{%
    \setlength{\gwa@closingneed}{0pt}%
    \ifgwa@phonenotice
    \else
        \ifgwa@hasattachments
        \else
            \setbox\gwa@recordbox=\vbox{\FooterRecord}%
            \setlength{\gwa@closingneed}{%
                \dimexpr\ht\gwa@recordbox+\dp\gwa@recordbox+\SignatureRecordGap\relax}%
        \fi
    \fi
    \gwa@placeclosing{\SignatureContent}%
}

% ========== 主文档生成命令 ==========
\newcommand{\makeletter}{%
    \thispagestyle{firstpage}
    \zihao{3}

    \gwa@bodyspacing

    % 抬头：白头件“密级 + 空 10 行”、会议议程“密级 + 空 1 行”、普通公文仅密级、函稿/电话通知“红头 + 函号”。
    \ifgwa@whitepaper
        \WhitePaperHeader
    \else\ifgwa@meetingagenda
        \MeetingAgendaHeader
    \else\ifgwa@plaindocument
        \PlainDocumentHeader
    \else
        \DocumentHeader
        \vspace{\BodyBaselineSkip}
    \fi
    \fi
    \fi

    % 标题（生成器按字数与 jieba 排布后写入 \TitleContent，未写入时回退 \DocumentTitle）
    {%
        \centering
        \ifthenelse{\equal{\TitleContent}{}}%
            {{\bs\enbt\zihao{2}\setlength{\baselineskip}{\BodyBaselineSkip}\DocumentTitle{}}}%
            {\TitleContent}%
        \par
    }

    \ifgwa@meetingagenda
        % 会议议程：标题空一行后直接正文，无呈报领导、无落款、无版记。
        \vspace{\baselineskip}
        {\zihao{3}
            \gwa@bodyspacing
            \MainContent
        }
    \else\ifgwa@plaindocument
        % 普通公文：标题后直接排正文及可选附件，无主送、落款和版记。
        \vspace{\BodyBaselineSkip}
        {\zihao{3}
            \gwa@bodyspacing
            \MainContent
        }
        \ifgwa@hasattachments
            \clearpage
            \AttachmentContent
        \fi
    \else
        % 呈报领导（白头件）／主送机关（函稿、电话通知）
        \vspace{\BodyBaselineSkip}
        {\noindent \zihao{3}\kai\enkai\Recipient{}：}

        % 正文
        {\zihao{3}
            \gwa@bodyspacing
            \MainContent
        }

        \ifgwa@whitepaper
            % 白头件：按落款实际高度判断“此页无正文”；无版记，落款靠左排、右侧留签字空间。
            \WhitePaperClosing
        \else
            % 按落款（无附件时连同版记）的实际高度判断正文末页放不放得下；
            % 另起的落款页自动标注“此页无正文”。
            \LetterClosing

            % 附件与正文分区，始终从新页开始；版记置于全文（含附件）末尾。
            \ifgwa@hasattachments
                \clearpage
                \AttachmentContent
            \fi
            \ifgwa@phonenotice
            \else
                \FooterRecord
            \fi
        \fi
    \fi
    \fi
}

\endinput
